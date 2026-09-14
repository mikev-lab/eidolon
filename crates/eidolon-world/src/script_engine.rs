//! Sandboxed Gameplay Scripting Virtual Machine with Instruction Gas Metering.
//!
//! Provides a secure, zero-dependency bytecode interpreter executing quest logic,
//! achievement triggers, and dynamic game rules with deterministic gas bounds.

/// Maximum operand stack depth.
pub const VM_STACK_CAPACITY: usize = 128;
/// Maximum local variable registers per script execution context.
pub const VM_VARS_CAPACITY: usize = 32;
/// Default gas allocation per script execution slice.
pub const DEFAULT_GAS_LIMIT: u64 = 10_000;

/// Bytecode instructions for the sandboxed virtual machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpCode {
    /// Pushes a 64-bit integer constant onto the stack.
    Push(i64),
    /// Discards the top value on the stack.
    Pop,
    /// Duplicates the top value on the stack.
    Dup,
    /// Swaps the top two values on the stack.
    Swap,
    /// Adds top two values: (b + a).
    Add,
    /// Subtracts top value from second value: (b - a).
    Sub,
    /// Multiplies top two values: (b * a).
    Mul,
    /// Divides second value by top value: (b / a).
    Div,
    /// Computes modulo of second value by top value: (b % a).
    Mod,
    /// Equality comparison: 1 if b == a else 0.
    Eq,
    /// Non-equality comparison: 1 if b != a else 0.
    Neq,
    /// Less-than comparison: 1 if b < a else 0.
    Lt,
    /// Less-than-or-equal comparison: 1 if b <= a else 0.
    Lte,
    /// Greater-than comparison: 1 if b > a else 0.
    Gt,
    /// Greater-than-or-equal comparison: 1 if b >= a else 0.
    Gte,
    /// Unconditional jump to target instruction index.
    Jump(usize),
    /// Pops condition; jumps to target index if condition != 0.
    JumpIfTrue(usize),
    /// Pops condition; jumps to target index if condition == 0.
    JumpIfFalse(usize),
    /// Loads a value from variable register index.
    LoadVar(u16),
    /// Pops top value and stores into variable register index.
    StoreVar(u16),
    /// Invokes host syscall: pops argument, invokes syscall_id, pushes return value.
    CallSyscall(u16),
    /// Terminates execution, returning top stack value as exit code.
    Halt,
}

/// Errors occurring during virtual machine bytecode execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmError {
    /// Execution gas limit exhausted (prevents infinite loops).
    GasExhausted,
    /// Operand stack exceeded fixed capacity.
    StackOverflow,
    /// Attempted to pop an empty stack.
    StackUnderflow,
    /// Division or modulo by zero.
    DivisionByZero,
    /// Jump instruction points outside bytecode bounds.
    InvalidJumpTarget,
    /// Variable index exceeds register bounds.
    InvalidVariableIndex,
    /// Host syscall failed with error code.
    SyscallFailed(u16),
    /// Bytecode completed without explicit Halt instruction.
    UnexpectedEndOfBytecode,
}

impl core::fmt::Display for VmError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::GasExhausted => write!(f, "Execution gas limit exhausted"),
            Self::StackOverflow => write!(f, "Operand stack overflow"),
            Self::StackUnderflow => write!(f, "Operand stack underflow"),
            Self::DivisionByZero => write!(f, "Division by zero"),
            Self::InvalidJumpTarget => write!(f, "Invalid jump target index"),
            Self::InvalidVariableIndex => write!(f, "Invalid variable index"),
            Self::SyscallFailed(code) => write!(f, "Host syscall failed with code {code}"),
            Self::UnexpectedEndOfBytecode => write!(f, "Unexpected end of bytecode"),
        }
    }
}

impl std::error::Error for VmError {}

/// Host interface trait enabling the VM to interact with authoritative world systems.
pub trait HostEnvironment {
    /// Handles a syscall invoked by the script.
    fn handle_syscall(&mut self, syscall_id: u16, arg: i64) -> Result<i64, u16>;
}

/// Default mock host environment for testing and pure computation.
#[derive(Debug, Default)]
pub struct MockHostEnvironment {
    /// Syscall invocation log: (syscall_id, argument).
    pub invocations: Vec<(u16, i64)>,
    /// Configured responses for syscall IDs.
    pub responses: [i64; 16],
}

impl HostEnvironment for MockHostEnvironment {
    fn handle_syscall(&mut self, syscall_id: u16, arg: i64) -> Result<i64, u16> {
        self.invocations.push((syscall_id, arg));
        if (syscall_id as usize) < self.responses.len() {
            Ok(self.responses[syscall_id as usize])
        } else {
            Ok(0)
        }
    }
}

/// Sandboxed bytecode virtual machine.
#[derive(Debug)]
pub struct ScriptVm {
    stack: [i64; VM_STACK_CAPACITY],
    stack_ptr: usize,
    vars: [i64; VM_VARS_CAPACITY],
    gas_remaining: u64,
}

impl Default for ScriptVm {
    fn default() -> Self {
        Self {
            stack: [0; VM_STACK_CAPACITY],
            stack_ptr: 0,
            vars: [0; VM_VARS_CAPACITY],
            gas_remaining: 0,
        }
    }
}

impl ScriptVm {
    /// Creates a new virtual machine.
    pub fn new() -> Self {
        Self::default()
    }

    /// Resets the operand stack and variable registers.
    pub fn reset(&mut self) {
        self.stack_ptr = 0;
        self.vars.fill(0);
        self.gas_remaining = 0;
    }

    /// Pushes an integer onto the stack.
    #[inline]
    fn push(&mut self, val: i64) -> Result<(), VmError> {
        if self.stack_ptr >= VM_STACK_CAPACITY {
            return Err(VmError::StackOverflow);
        }
        self.stack[self.stack_ptr] = val;
        self.stack_ptr += 1;
        Ok(())
    }

    /// Pops an integer from the stack.
    #[inline]
    fn pop(&mut self) -> Result<i64, VmError> {
        if self.stack_ptr == 0 {
            return Err(VmError::StackUnderflow);
        }
        self.stack_ptr -= 1;
        Ok(self.stack[self.stack_ptr])
    }

    /// Executes a bytecode program within the specified gas limit.
    pub fn execute<H: HostEnvironment>(
        &mut self,
        bytecode: &[OpCode],
        max_gas: u64,
        host: &mut H,
    ) -> Result<i64, VmError> {
        self.reset();
        self.gas_remaining = max_gas;

        let mut pc = 0;

        while pc < bytecode.len() {
            // Deduct 1 unit of gas per instruction
            if self.gas_remaining == 0 {
                return Err(VmError::GasExhausted);
            }
            self.gas_remaining -= 1;

            match bytecode[pc] {
                OpCode::Push(val) => {
                    self.push(val)?;
                    pc += 1;
                }
                OpCode::Pop => {
                    self.pop()?;
                    pc += 1;
                }
                OpCode::Dup => {
                    let top = self.pop()?;
                    self.push(top)?;
                    self.push(top)?;
                    pc += 1;
                }
                OpCode::Swap => {
                    let a = self.pop()?;
                    let b = self.pop()?;
                    self.push(a)?;
                    self.push(b)?;
                    pc += 1;
                }
                OpCode::Add => {
                    let a = self.pop()?;
                    let b = self.pop()?;
                    self.push(b.saturating_add(a))?;
                    pc += 1;
                }
                OpCode::Sub => {
                    let a = self.pop()?;
                    let b = self.pop()?;
                    self.push(b.saturating_sub(a))?;
                    pc += 1;
                }
                OpCode::Mul => {
                    let a = self.pop()?;
                    let b = self.pop()?;
                    self.push(b.saturating_mul(a))?;
                    pc += 1;
                }
                OpCode::Div => {
                    let a = self.pop()?;
                    let b = self.pop()?;
                    if a == 0 {
                        return Err(VmError::DivisionByZero);
                    }
                    self.push(b.checked_div(a).unwrap_or(0))?;
                    pc += 1;
                }
                OpCode::Mod => {
                    let a = self.pop()?;
                    let b = self.pop()?;
                    if a == 0 {
                        return Err(VmError::DivisionByZero);
                    }
                    self.push(b.checked_rem(a).unwrap_or(0))?;
                    pc += 1;
                }
                OpCode::Eq => {
                    let a = self.pop()?;
                    let b = self.pop()?;
                    self.push(if b == a { 1 } else { 0 })?;
                    pc += 1;
                }
                OpCode::Neq => {
                    let a = self.pop()?;
                    let b = self.pop()?;
                    self.push(if b != a { 1 } else { 0 })?;
                    pc += 1;
                }
                OpCode::Lt => {
                    let a = self.pop()?;
                    let b = self.pop()?;
                    self.push(if b < a { 1 } else { 0 })?;
                    pc += 1;
                }
                OpCode::Lte => {
                    let a = self.pop()?;
                    let b = self.pop()?;
                    self.push(if b <= a { 1 } else { 0 })?;
                    pc += 1;
                }
                OpCode::Gt => {
                    let a = self.pop()?;
                    let b = self.pop()?;
                    self.push(if b > a { 1 } else { 0 })?;
                    pc += 1;
                }
                OpCode::Gte => {
                    let a = self.pop()?;
                    let b = self.pop()?;
                    self.push(if b >= a { 1 } else { 0 })?;
                    pc += 1;
                }
                OpCode::Jump(target) => {
                    if target >= bytecode.len() {
                        return Err(VmError::InvalidJumpTarget);
                    }
                    pc = target;
                }
                OpCode::JumpIfTrue(target) => {
                    if target >= bytecode.len() {
                        return Err(VmError::InvalidJumpTarget);
                    }
                    let cond = self.pop()?;
                    if cond != 0 {
                        pc = target;
                    } else {
                        pc += 1;
                    }
                }
                OpCode::JumpIfFalse(target) => {
                    if target >= bytecode.len() {
                        return Err(VmError::InvalidJumpTarget);
                    }
                    let cond = self.pop()?;
                    if cond == 0 {
                        pc = target;
                    } else {
                        pc += 1;
                    }
                }
                OpCode::LoadVar(idx) => {
                    if (idx as usize) >= VM_VARS_CAPACITY {
                        return Err(VmError::InvalidVariableIndex);
                    }
                    self.push(self.vars[idx as usize])?;
                    pc += 1;
                }
                OpCode::StoreVar(idx) => {
                    if (idx as usize) >= VM_VARS_CAPACITY {
                        return Err(VmError::InvalidVariableIndex);
                    }
                    let val = self.pop()?;
                    self.vars[idx as usize] = val;
                    pc += 1;
                }
                OpCode::CallSyscall(syscall_id) => {
                    let arg = self.pop()?;
                    match host.handle_syscall(syscall_id, arg) {
                        Ok(ret) => {
                            self.push(ret)?;
                            pc += 1;
                        }
                        Err(err_code) => return Err(VmError::SyscallFailed(err_code)),
                    }
                }
                OpCode::Halt => {
                    // Return top stack value if present, else 0
                    let ret = self.pop().unwrap_or(0);
                    return Ok(ret);
                }
            }
        }

        Err(VmError::UnexpectedEndOfBytecode)
    }
}

/// Player quest objective state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuestStatus {
    /// Quest has not been accepted.
    Unassigned,
    /// Quest is active with objective counts.
    Active {
        /// Objective completion counts.
        progress: [u32; 4],
    },
    /// Quest has been successfully turned in.
    Completed,
    /// Quest was abandoned or failed.
    Failed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_arithmetic_computation() {
        let mut vm = ScriptVm::new();
        let mut host = MockHostEnvironment::default();

        // Calculate (10 + 20) * 3 - 5 = 85
        let bytecode = [
            OpCode::Push(10),
            OpCode::Push(20),
            OpCode::Add,
            OpCode::Push(3),
            OpCode::Mul,
            OpCode::Push(5),
            OpCode::Sub,
            OpCode::Halt,
        ];

        let res = vm.execute(&bytecode, 100, &mut host).unwrap();
        assert_eq!(res, 85);
    }

    #[test]
    fn test_gas_exhaustion_infinite_loop() {
        let mut vm = ScriptVm::new();
        let mut host = MockHostEnvironment::default();

        // Infinite loop: Jump(0)
        let bytecode = [OpCode::Jump(0)];

        // Run with 50 gas
        let err = vm.execute(&bytecode, 50, &mut host);
        assert_eq!(err, Err(VmError::GasExhausted));
    }

    #[test]
    fn test_host_syscall_and_conditional_branching() {
        let mut vm = ScriptVm::new();
        let mut host = MockHostEnvironment::default();
        host.responses[1] = 10; // Syscall 1 (GetKillCount) returns 10

        // Program:
        // 0: CallSyscall(1, 0) -> pushes 10
        // 1: Push(10)
        // 2: Gte -> (10 >= 10) = 1
        // 3: JumpIfFalse(6) -> skip reward if < 10
        // 4: CallSyscall(2, 500) -> GrantReward(500)
        // 5: Halt
        // 6: Push(0)
        // 7: Halt
        let bytecode = [
            OpCode::Push(0),
            OpCode::CallSyscall(1),
            OpCode::Push(10),
            OpCode::Gte,
            OpCode::JumpIfFalse(7),
            OpCode::Push(500),
            OpCode::CallSyscall(2),
            OpCode::Halt,
            OpCode::Push(0),
            OpCode::Halt,
        ];

        let res = vm.execute(&bytecode, 100, &mut host).unwrap();
        assert_eq!(host.invocations.len(), 2);
        assert_eq!(host.invocations[0], (1, 0));
        assert_eq!(host.invocations[1], (2, 500));
        assert_eq!(res, 0); // Default response for syscall 2
    }

    #[test]
    fn test_division_by_zero_safety() {
        let mut vm = ScriptVm::new();
        let mut host = MockHostEnvironment::default();

        let bytecode = [OpCode::Push(10), OpCode::Push(0), OpCode::Div, OpCode::Halt];
        let err = vm.execute(&bytecode, 100, &mut host);
        assert_eq!(err, Err(VmError::DivisionByZero));
    }
}
