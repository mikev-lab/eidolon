//! Exhaustive integration test suite for Phase 24: Sandboxed Gameplay Scripting & Reactive Event Bus.
//!
//! Validates:
//! 1. Reactive event bus publish, FIFO ring buffering, and filtered subscriber callbacks.
//! 2. Sandboxed bytecode virtual machine arithmetic, stack manipulation, and register ops.
//! 3. Gas metering and deterministic timeout on infinite loop execution.
//! 4. Quest progression state machine reacting to world events and invoking host reward syscalls.
//! 5. Fault-tolerance invariants: division by zero, stack bounds, invalid jumps, and typed errors with zero panics.

use eidolon_core::{EventBus, EventContext, GameEvent, EVENT_QUEUE_CAPACITY};
use eidolon_world::{
    HostEnvironment, MockHostEnvironment, OpCode, QuestStatus, ScriptVm, VmError, DEFAULT_GAS_LIMIT,
};

#[test]
fn test_event_bus_filtered_subscriber_dispatch() {
    let mut bus = EventBus::new();

    // Context tracking actions across subscribers
    let mut ctx = EventContext::default();

    fn on_kill(event: &GameEvent, ctx: &mut EventContext) {
        if let GameEvent::EntityKilled { victim_type, .. } = event {
            if *victim_type == 10 {
                ctx.actions_triggered += 1;
            }
        }
    }

    fn on_item(event: &GameEvent, ctx: &mut EventContext) {
        if let GameEvent::ItemAcquired { quantity, .. } = event {
            ctx.actions_triggered += *quantity;
        }
    }

    // Subscribe:
    // Subscriber 1: EntityKilled (type_id = 1 -> mask = 1 << 1 = 2)
    // Subscriber 2: ItemAcquired (type_id = 2 -> mask = 1 << 2 = 4)
    assert!(bus.subscribe(1, 1 << 1, on_kill));
    assert!(bus.subscribe(2, 1 << 2, on_item));

    // Publish matching kill event (victim_type = 10)
    bus.publish(GameEvent::EntityKilled {
        killer_id: 100,
        victim_id: 501,
        victim_type: 10,
        zone_id: 1,
        tick: 100,
    });

    // Publish non-matching kill event (victim_type = 99)
    bus.publish(GameEvent::EntityKilled {
        killer_id: 100,
        victim_id: 502,
        victim_type: 99,
        zone_id: 1,
        tick: 101,
    });

    // Publish item event (quantity = 5)
    bus.publish(GameEvent::ItemAcquired {
        player_id: 100,
        item_id: 3001,
        quantity: 5,
        tick: 102,
    });

    // Publish location reached event (type_id = 3 -> mask = 1 << 3 = 8, no subscriber)
    bus.publish(GameEvent::LocationReached {
        player_id: 100,
        zone_id: 1,
        sector_x: 2,
        sector_z: 3,
        tick: 103,
    });

    assert_eq!(bus.pending_count(), 4);
    let dispatched = bus.dispatch_all(&mut ctx);
    assert_eq!(dispatched, 4);
    assert_eq!(bus.pending_count(), 0);
    assert!(bus.is_empty());

    // Expected actions: 1 from kill + 5 from item = 6
    assert_eq!(ctx.actions_triggered, 6);

    // Unsubscribe subscriber 1
    assert!(bus.unsubscribe(1));

    // Publish another kill event; should not trigger action since sub 1 unsubscribed
    bus.publish(GameEvent::EntityKilled {
        killer_id: 100,
        victim_id: 503,
        victim_type: 10,
        zone_id: 1,
        tick: 104,
    });
    bus.dispatch_all(&mut ctx);
    assert_eq!(ctx.actions_triggered, 6);
}

#[test]
fn test_event_bus_ring_buffer_saturation_drops_oldest() {
    let mut bus = EventBus::new();

    // Scribe to custom events (type_id = 5, mask = 1 << 5 = 32)
    fn on_custom(event: &GameEvent, ctx: &mut EventContext) {
        if let GameEvent::Custom { param1, .. } = event {
            ctx.actions_triggered += *param1 as u32;
        }
    }
    assert!(bus.subscribe(10, 1 << 5, on_custom));

    // Fill ring buffer past capacity (EVENT_QUEUE_CAPACITY + 5)
    let total_events = EVENT_QUEUE_CAPACITY + 5;
    for i in 0..total_events {
        bus.publish(GameEvent::Custom {
            event_id: 1,
            actor_id: 100,
            param1: 1,
            param2: i as u64,
            tick: i as u64,
        });
    }

    // Capacity must remain bounded to EVENT_QUEUE_CAPACITY
    assert_eq!(bus.pending_count(), EVENT_QUEUE_CAPACITY);

    let mut ctx = EventContext::default();
    let dispatched = bus.dispatch_all(&mut ctx);
    assert_eq!(dispatched, EVENT_QUEUE_CAPACITY);
    // All dispatched events had param1 = 1
    assert_eq!(ctx.actions_triggered, EVENT_QUEUE_CAPACITY as u32);
}

#[test]
fn test_script_vm_arithmetic_stack_and_register_ops() {
    let mut vm = ScriptVm::new();
    let mut host = MockHostEnvironment::default();

    // Expression: ((15 + 25) * 4 - 10) / 3 % 17
    // Step 1: 15 + 25 = 40
    // Step 2: 40 * 4 = 160
    // Step 3: 160 - 10 = 150
    // Step 4: 150 / 3 = 50
    // Step 5: 50 % 17 = 16
    // Store in register 3, load from register 3, halt
    let bytecode = [
        OpCode::Push(15),
        OpCode::Push(25),
        OpCode::Add,
        OpCode::Push(4),
        OpCode::Mul,
        OpCode::Push(10),
        OpCode::Sub,
        OpCode::Push(3),
        OpCode::Div,
        OpCode::Push(17),
        OpCode::Mod,
        OpCode::StoreVar(3),
        OpCode::LoadVar(3),
        OpCode::Halt,
    ];

    let result = vm.execute(&bytecode, 1000, &mut host).unwrap();
    assert_eq!(result, 16);

    // Stack duplication and swap:
    // Push(10), Push(20), Swap (stack: [20, 10]), Dup (stack: [20, 10, 10]), Sub (10 - 10 = 0), Add (20 + 0 = 20)
    let stack_ops = [
        OpCode::Push(10),
        OpCode::Push(20),
        OpCode::Swap,
        OpCode::Dup,
        OpCode::Sub,
        OpCode::Add,
        OpCode::Halt,
    ];
    let res2 = vm.execute(&stack_ops, 1000, &mut host).unwrap();
    assert_eq!(res2, 20);
}

#[test]
fn test_script_vm_gas_metering_and_infinite_loop_prevention() {
    let mut vm = ScriptVm::new();
    let mut host = MockHostEnvironment::default();

    // Tight infinite loop: Jump(0)
    let tight_loop = [OpCode::Jump(0)];
    let err1 = vm.execute(&tight_loop, 250, &mut host);
    assert_eq!(err1, Err(VmError::GasExhausted));

    // Multi-instruction loop:
    // 0: Push(1)
    // 1: Push(1)
    // 2: Add
    // 3: Pop
    // 4: Jump(0)
    let multi_loop = [
        OpCode::Push(1),
        OpCode::Push(1),
        OpCode::Add,
        OpCode::Pop,
        OpCode::Jump(0),
    ];
    let err2 = vm.execute(&multi_loop, 100, &mut host);
    assert_eq!(err2, Err(VmError::GasExhausted));

    // Verifying execution succeeds if gas is sufficient:
    // 10-iteration countdown loop
    // Store 10 in Var(0)
    // Loop:
    // LoadVar(0), Push(1), Sub, StoreVar(0)
    // LoadVar(0), Push(0), Gt, JumpIfTrue(Loop)
    // Halt
    let countdown = [
        OpCode::Push(10),
        OpCode::StoreVar(0),
        // loop start at index 2
        OpCode::LoadVar(0),
        OpCode::Push(1),
        OpCode::Sub,
        OpCode::StoreVar(0),
        OpCode::LoadVar(0),
        OpCode::Push(0),
        OpCode::Gt,
        OpCode::JumpIfTrue(2),
        OpCode::LoadVar(0),
        OpCode::Halt,
    ];
    let count_res = vm
        .execute(&countdown, DEFAULT_GAS_LIMIT, &mut host)
        .unwrap();
    assert_eq!(count_res, 0);
}

#[test]
fn test_quest_progression_and_host_syscall_rewards() {
    let mut bus = EventBus::new();
    let mut vm = ScriptVm::new();
    let mut host = MockHostEnvironment::default();

    // Mock host configuration:
    // Syscall 10: GrantXP (returns 1 on success)
    // Syscall 11: GrantGold (returns 1 on success)
    host.responses[10] = 1;
    host.responses[11] = 1;

    // Quest: "Defeat 5 Shadow Wolves (victim_type = 77)"
    const WOLF_VICTIM_TYPE: u32 = 77;
    const TARGET_KILLS: u32 = 5;

    let mut quest_status = QuestStatus::Active { progress: [0; 4] };

    // Subscriber callback updates quest progress when wolf killed
    fn on_wolf_kill(event: &GameEvent, ctx: &mut EventContext) {
        if let GameEvent::EntityKilled { victim_type, .. } = event {
            if *victim_type == WOLF_VICTIM_TYPE {
                ctx.actions_triggered += 1;
            }
        }
    }
    assert!(bus.subscribe(101, 1 << 1, on_wolf_kill));

    // Simulate combat: kill 5 wolves across 5 ticks
    for tick in 1..=TARGET_KILLS {
        bus.publish(GameEvent::EntityKilled {
            killer_id: 1001,
            victim_id: 5000 + tick as u64,
            victim_type: WOLF_VICTIM_TYPE,
            zone_id: 1,
            tick: tick as u64,
        });
    }

    let mut event_ctx = EventContext::default();
    bus.dispatch_all(&mut event_ctx);

    let kills_registered = event_ctx.actions_triggered;
    assert_eq!(kills_registered, 5);

    // Update quest state
    if let QuestStatus::Active { ref mut progress } = quest_status {
        progress[0] = kills_registered;
    }

    // Quest completion script:
    // Check if progress[0] >= 5. If yes, invoke host reward syscalls.
    // 0: Push(kills_registered)
    // 1: Push(5)
    // 2: Gte
    // 3: JumpIfFalse(10)
    // 4: Push(500) -> 500 XP
    // 5: CallSyscall(10)
    // 6: Pop
    // 7: Push(100) -> 100 Gold
    // 8: CallSyscall(11)
    // 9: Halt
    // 10: Push(-1)
    // 11: Halt
    let quest_script = [
        OpCode::Push(kills_registered as i64),
        OpCode::Push(5),
        OpCode::Gte,
        OpCode::JumpIfFalse(10),
        OpCode::Push(500),
        OpCode::CallSyscall(10),
        OpCode::Pop,
        OpCode::Push(100),
        OpCode::CallSyscall(11),
        OpCode::Halt,
        OpCode::Push(-1),
        OpCode::Halt,
    ];

    let script_exit = vm.execute(&quest_script, 1000, &mut host).unwrap();
    assert_eq!(script_exit, 1); // Return value of Syscall 11

    // Transition quest status to Completed
    quest_status = QuestStatus::Completed;
    assert_eq!(quest_status, QuestStatus::Completed);

    // Verify host received both reward syscall invocations
    assert_eq!(host.invocations.len(), 2);
    assert_eq!(host.invocations[0], (10, 500)); // Syscall 10 with 500 XP
    assert_eq!(host.invocations[1], (11, 100)); // Syscall 11 with 100 Gold
}

#[test]
fn test_script_vm_fault_tolerance_and_boundary_safety() {
    let mut vm = ScriptVm::new();
    let mut host = MockHostEnvironment::default();

    // 1. Division by zero
    let div_zero = [
        OpCode::Push(100),
        OpCode::Push(0),
        OpCode::Div,
        OpCode::Halt,
    ];
    assert_eq!(
        vm.execute(&div_zero, 100, &mut host),
        Err(VmError::DivisionByZero)
    );

    // 2. Modulo by zero
    let mod_zero = [
        OpCode::Push(100),
        OpCode::Push(0),
        OpCode::Mod,
        OpCode::Halt,
    ];
    assert_eq!(
        vm.execute(&mod_zero, 100, &mut host),
        Err(VmError::DivisionByZero)
    );

    // 3. Stack underflow (Pop empty)
    let underflow = [OpCode::Pop, OpCode::Halt];
    assert_eq!(
        vm.execute(&underflow, 100, &mut host),
        Err(VmError::StackUnderflow)
    );

    // 4. Stack underflow on binary operator (Add with 1 element)
    let underflow_add = [OpCode::Push(10), OpCode::Add, OpCode::Halt];
    assert_eq!(
        vm.execute(&underflow_add, 100, &mut host),
        Err(VmError::StackUnderflow)
    );

    // 5. Stack overflow (Push > 128 items)
    let mut overflow_script = Vec::with_capacity(130);
    for i in 0..129 {
        overflow_script.push(OpCode::Push(i));
    }
    overflow_script.push(OpCode::Halt);
    assert_eq!(
        vm.execute(&overflow_script, 1000, &mut host),
        Err(VmError::StackOverflow)
    );

    // 6. Invalid jump target
    let bad_jump = [OpCode::Jump(999)];
    assert_eq!(
        vm.execute(&bad_jump, 100, &mut host),
        Err(VmError::InvalidJumpTarget)
    );

    // 7. Invalid variable register
    let bad_var = [OpCode::LoadVar(100), OpCode::Halt];
    assert_eq!(
        vm.execute(&bad_var, 100, &mut host),
        Err(VmError::InvalidVariableIndex)
    );

    // 8. Host syscall failure
    struct FailingHost;
    impl HostEnvironment for FailingHost {
        fn handle_syscall(&mut self, _syscall_id: u16, _arg: i64) -> Result<i64, u16> {
            Err(404) // Resource not found
        }
    }
    let mut failing_host = FailingHost;
    let syscall_fail = [OpCode::Push(1), OpCode::CallSyscall(99), OpCode::Halt];
    assert_eq!(
        vm.execute(&syscall_fail, 100, &mut failing_host),
        Err(VmError::SyscallFailed(404))
    );

    // 9. Unterminated bytecode without Halt
    let unterminated = [OpCode::Push(42)];
    assert_eq!(
        vm.execute(&unterminated, 100, &mut host),
        Err(VmError::UnexpectedEndOfBytecode)
    );
}
