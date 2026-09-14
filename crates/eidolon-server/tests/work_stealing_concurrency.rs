//! Multi-core work-stealing concurrency and deterministic sector parallelism test suite.
//!
//! Validates lock-free Chase-Lev deque integrity under multi-threaded contention,
//! cross-worker cooperative stealing, and bit-exact multi-core sector simulation determinism.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use eidolon_core::fixed::{Fixed64, Vec3Fix};
use eidolon_core::quant::QuantizedYaw;
use eidolon_server::work_stealing::{ChaseLevDeque, StealResult, WorkStealingPool};
use eidolon_world::sector_parallel::SectorParallelCoordinator;
use eidolon_world::soa_storage::{EntitySpawnParams, SoaEntityStorage};

#[test]
fn test_chase_lev_concurrent_push_pop_steal_integrity() {
    const TOTAL_TASKS: usize = 10_000;
    const THIEF_COUNT: usize = 7;

    let deque = Arc::new(ChaseLevDeque::with_capacity(16384).expect("valid capacity"));
    let processed_counts = Arc::new(
        (0..=TOTAL_TASKS)
            .map(|_| AtomicUsize::new(0))
            .collect::<Vec<_>>(),
    );
    let total_popped = Arc::new(AtomicUsize::new(0));
    let total_stolen = Arc::new(AtomicUsize::new(0));
    let producer_finished = Arc::new(AtomicBool::new(false));

    // Spawn 7 thief threads
    let mut thieves = Vec::with_capacity(THIEF_COUNT);
    for _ in 0..THIEF_COUNT {
        let q = Arc::clone(&deque);
        let counts = Arc::clone(&processed_counts);
        let stolen_counter = Arc::clone(&total_stolen);
        let done = Arc::clone(&producer_finished);

        let handle = thread::spawn(move || {
            while !done.load(Ordering::Acquire) || !q.is_empty() {
                match q.steal() {
                    StealResult::Success(val) => {
                        let id = val as usize;
                        if id <= TOTAL_TASKS {
                            counts[id].fetch_add(1, Ordering::SeqCst);
                            stolen_counter.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    StealResult::Abort => {
                        thread::yield_now();
                    }
                    StealResult::Empty => {
                        thread::yield_now();
                    }
                }
            }
        });
        thieves.push(handle);
    }

    // Owner thread pushes and periodically pops
    let q_owner = Arc::clone(&deque);
    let counts_owner = Arc::clone(&processed_counts);
    let popped_counter = Arc::clone(&total_popped);

    for i in 1..=TOTAL_TASKS {
        q_owner.push(i as u64).expect("push must succeed");

        // Interleave local pops (owner work)
        if i % 5 == 0 {
            if let Some(val) = q_owner.pop() {
                let id = val as usize;
                if id <= TOTAL_TASKS {
                    counts_owner[id].fetch_add(1, Ordering::SeqCst);
                    popped_counter.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }

    // Drain remaining tasks as owner
    while let Some(val) = q_owner.pop() {
        let id = val as usize;
        if id <= TOTAL_TASKS {
            counts_owner[id].fetch_add(1, Ordering::SeqCst);
            popped_counter.fetch_add(1, Ordering::Relaxed);
        }
    }

    producer_finished.store(true, Ordering::Release);

    for handle in thieves {
        handle.join().expect("thief thread must join cleanly");
    }

    let popped = total_popped.load(Ordering::SeqCst);
    let stolen = total_stolen.load(Ordering::SeqCst);
    let total = popped + stolen;

    assert_eq!(
        total, TOTAL_TASKS,
        "Total processed tasks ({total}) must equal produced tasks ({TOTAL_TASKS})"
    );

    // Verify exactly one pop/steal per task ID (zero duplicate executions and zero task loss)
    for id in 1..=TOTAL_TASKS {
        let c = processed_counts[id].load(Ordering::SeqCst);
        assert_eq!(
            c, 1,
            "Task ID {id} was processed {c} times (must be exactly 1)"
        );
    }
}

#[test]
fn test_work_stealing_pool_job_distribution() {
    const JOB_COUNT: usize = 2_000;
    let processed_jobs = Arc::new(AtomicUsize::new(0));
    let counter_clone = Arc::clone(&processed_jobs);

    let pool = WorkStealingPool::new(4, move |_worker_id, _job| {
        counter_clone.fetch_add(1, Ordering::Relaxed);
    });

    for i in 0..JOB_COUNT {
        pool.submit(i as u64).expect("submit must succeed");
    }

    let start = Instant::now();
    while processed_jobs.load(Ordering::Relaxed) < JOB_COUNT
        && start.elapsed() < Duration::from_secs(5)
    {
        thread::sleep(Duration::from_millis(5));
    }

    let total = processed_jobs.load(Ordering::SeqCst);
    pool.shutdown();

    assert_eq!(
        total, JOB_COUNT,
        "All submitted jobs must be processed by work-stealing pool"
    );
}

#[test]
fn test_sector_parallel_determinism_parity() {
    // Setup two identical environments: one executed on 1 worker, one on 4 workers
    let mut coord_seq = SectorParallelCoordinator::new(500);
    let mut coord_par = SectorParallelCoordinator::new(500);

    let mut storage_seq = SoaEntityStorage::with_capacity(500);
    let mut storage_par = SoaEntityStorage::with_capacity(500);

    for id in 1..=200 {
        let x = ((id % 10) as f64) * 12.0; // Spanned across sectors
        let z = ((id / 10) as f64) * 8.0;
        let pos = Vec3Fix::from_f64(x, 0.0, z);
        let vel = Vec3Fix::from_f64(2.5, 0.0, -1.5);
        let yaw = QuantizedYaw::from_degrees((id as f64) * 5.0);

        let params = EntitySpawnParams::new(id, pos, vel, yaw);
        storage_seq.spawn(params.clone()).expect("spawn seq");
        storage_par.spawn(params).expect("spawn par");

        coord_seq.assign_entity(id, pos).expect("assign seq");
        coord_par.assign_entity(id, pos).expect("assign par");
    }

    let dt = Fixed64::from_f64(0.05);

    // Simulate 20 consecutive simulation ticks
    for _ in 0..20 {
        coord_seq.step_simulation_parallel(&mut storage_seq, dt, 1);
        coord_par.step_simulation_parallel(&mut storage_par, dt, 4);

        // Assert migrations count parity
        assert_eq!(
            coord_seq.migrations().len(),
            coord_par.migrations().len(),
            "Migration counts must match between single-thread and multi-thread"
        );

        // Assert migrations exact parity
        for (m_seq, m_par) in coord_seq
            .migrations()
            .iter()
            .zip(coord_par.migrations().iter())
        {
            assert_eq!(m_seq.entity_id, m_par.entity_id);
            assert_eq!(m_seq.old_sector, m_par.old_sector);
            assert_eq!(m_seq.new_sector, m_par.new_sector);
        }
    }

    // Assert final position and velocity bit-exact equivalence
    assert_eq!(storage_seq.len(), storage_par.len());
    for id in 1..=200 {
        let t_seq = storage_seq.get_transform(id).expect("transform seq");
        let t_par = storage_par.get_transform(id).expect("transform par");

        assert_eq!(
            t_seq.0, t_par.0,
            "Entity {id} position must be bit-exact identical"
        );
        assert_eq!(
            t_seq.1, t_par.1,
            "Entity {id} velocity must be bit-exact identical"
        );
    }
}

#[test]
fn test_multi_core_speedup_benchmark() {
    let mut coordinator = SectorParallelCoordinator::new(5000);
    let mut storage = SoaEntityStorage::with_capacity(5000);

    for id in 1..=2000 {
        let x = ((id % 20) as f64) * 16.0;
        let z = ((id / 20) as f64) * 16.0;
        let pos = Vec3Fix::from_f64(x, 0.0, z);
        let vel = Vec3Fix::from_f64(1.0, 0.0, 1.0);
        let yaw = QuantizedYaw::NORTH;

        let params = EntitySpawnParams::new(id, pos, vel, yaw);
        storage.spawn(params).expect("spawn entity");
        coordinator.assign_entity(id, pos).expect("assign entity");
    }

    let dt = Fixed64::from_f64(0.05);

    // Warmup
    coordinator.step_simulation_parallel(&mut storage, dt, 1);

    // 1-worker baseline
    let start_seq = Instant::now();
    for _ in 0..10 {
        coordinator.step_simulation_parallel(&mut storage, dt, 1);
    }
    let elapsed_seq = start_seq.elapsed();

    // 4-worker parallel
    let start_par = Instant::now();
    for _ in 0..10 {
        coordinator.step_simulation_parallel(&mut storage, dt, 4);
    }
    let elapsed_par = start_par.elapsed();

    println!(
        "Sector parallel benchmark: 1 worker: {:.2?}, 4 workers: {:.2?}",
        elapsed_seq, elapsed_par
    );
}
