// test_interpolator.cpp: Unit verification of C++17 Hermite spline & dead reckoning interpolator

#include "../EidolonInterpolator.hpp"
#include "../EidolonEntityManager.hpp"

#include <cassert>
#include <cmath>
#include <iostream>

int main() {
    using namespace eidolon;

    std::cout << "Testing EidolonInterpolator C++17..." << std::endl;

    InterpolatorConfig config;
    config.interpolation_delay_seconds = 0.05f; // 50ms
    config.max_extrapolation_seconds = 0.50f;
    config.snap_distance_threshold = 10.0f;
    config.error_decay_half_life = 0.10f;

    Interpolator interp(config);
    assert(!interp.is_initialized());

    // Initial snapshot at t=0
    TransformSnapshot s0;
    s0.x = 0.0f;
    s0.y = 0.0f;
    s0.z = 0.0f;
    s0.yaw_degrees = 0.0f;
    s0.velocity_x = 10.0f; // 10 m/s
    s0.velocity_z = 0.0f;
    s0.timestamp = 0.0;

    interp.reset(s0);
    assert(interp.is_initialized());

    // Evaluate at delta=0
    TransformSnapshot out0 = interp.evaluate(0.0f);
    assert(std::fabs(out0.x - 0.0f) < 0.001f);

    // Target update at t=0.05 (moved to x=0.5m)
    TransformSnapshot s1;
    s1.x = 0.5f;
    s1.y = 0.0f;
    s1.z = 0.0f;
    s1.yaw_degrees = 0.0f;
    s1.velocity_x = 10.0f;
    s1.velocity_z = 0.0f;
    s1.timestamp = 0.05;

    interp.update_target(s1);

    // Evaluate midway at 0.025s
    TransformSnapshot out_mid = interp.evaluate(0.025f);
    assert(out_mid.x > 0.0f && out_mid.x < 0.5f);

    // Evaluate at full interval (0.025s more -> 0.05s total)
    TransformSnapshot out_full = interp.evaluate(0.025f);
    assert(std::fabs(out_full.x - 0.5f) < 0.05f);

    // Test forward dead-reckoning extrapolation (extra 0.10s beyond delay)
    TransformSnapshot out_extrap = interp.evaluate(0.10f);
    assert(out_extrap.x > 0.5f); // Has extrapolated forward

    // Test instant snap on teleport (>10m)
    TransformSnapshot s_teleport;
    s_teleport.x = 100.0f; // 100m away
    s_teleport.y = 0.0f;
    s_teleport.z = 0.0f;
    s_teleport.yaw_degrees = 180.0f;
    s_teleport.velocity_x = 0.0f;
    s_teleport.velocity_z = 0.0f;
    s_teleport.timestamp = 0.20;

    interp.update_target(s_teleport);
    TransformSnapshot out_teleport = interp.evaluate(0.0f);
    assert(std::fabs(out_teleport.x - 100.0f) < 0.001f); // Snapped instantly

    // Test EntityManager
    std::cout << "Testing EidolonEntityManager C++17..." << std::endl;
    EntityManager manager(config);
    bool spawn_called = false;
    bool despawn_called = false;

    manager.set_spawn_callback([&](uint32_t id, uint8_t type, const TransformSnapshot&) {
        assert(id == 42);
        assert(type == 1);
        spawn_called = true;
    });

    manager.set_despawn_callback([&](uint32_t id) {
        assert(id == 42);
        despawn_called = true;
    });

    manager.on_entity_spawned(42, 1, s0);
    assert(spawn_called);
    assert(manager.has_entity(42));
    assert(manager.count() == 1);

    auto eval_res = manager.evaluate_entity(42, 0.016f); // 60 FPS frame (16.6ms)
    assert(eval_res.has_value());

    manager.on_entity_despawned(42);
    assert(despawn_called);
    assert(!manager.has_entity(42));
    assert(manager.count() == 0);

    std::cout << "All C++17 SDK tests passed successfully!" << std::endl;
    return 0;
}
