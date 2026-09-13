// EidolonInterpolator.hpp: High-Performance Hermite Spline & Dead Reckoning Interpolator
// Decouples client render loops (60/120/144/240 FPS) from 20 Hz server ticks.
// Strictly standard C++17 with zero third-party dependencies.

#ifndef EIDOLON_INTERPOLATOR_HPP
#define EIDOLON_INTERPOLATOR_HPP

#include <cmath>
#include <algorithm>
#include <cstdint>
#include <optional>

namespace eidolon {

/**
 * 3D transform snapshot received from server or extrapolated locally.
 */
struct TransformSnapshot {
    float x = 0.0f;
    float y = 0.0f;
    float z = 0.0f;
    float yaw_degrees = 0.0f;
    float velocity_x = 0.0f;
    float velocity_z = 0.0f;
    double timestamp = 0.0;
};

/**
 * Configuration parameters for client-side transform smoothing and extrapolation.
 */
struct InterpolatorConfig {
    // Interpolation delay in seconds (e.g. 0.05s = 50ms / 1 server tick buffer).
    float interpolation_delay_seconds = 0.05f;

    // Maximum duration in seconds to extrapolate with dead reckoning when packets stall.
    float max_extrapolation_seconds = 0.50f;

    // Displacement distance in meters that triggers an instant snap instead of smoothing.
    float snap_distance_threshold = 10.0f;

    // Half-life in seconds for decaying position error offsets.
    float error_decay_half_life = 0.10f;
};

/**
 * High-performance cubic Hermite spline interpolator with dead reckoning and error decay.
 */
class Interpolator {
public:
    explicit Interpolator(const InterpolatorConfig& config = InterpolatorConfig{})
        : m_config(config)
    {
    }

    /**
     * Resets the interpolator state to an explicit snapshot.
     */
    void reset(const TransformSnapshot& initial) {
        m_current = initial;
        m_target = initial;
        m_error_x = 0.0f;
        m_error_y = 0.0f;
        m_error_z = 0.0f;
        m_time_since_target = 0.0f;
        m_initialized = true;
    }

    /**
     * Ingests a new authoritative server transform update.
     */
    void update_target(const TransformSnapshot& new_snapshot) {
        if (!m_initialized) {
            reset(new_snapshot);
            return;
        }

        // Calculate discrepancy distance between current visual position and new target
        float dx = m_target.x - new_snapshot.x;
        float dy = m_target.y - new_snapshot.y;
        float dz = m_target.z - new_snapshot.z;
        float dist_sq = dx * dx + dy * dy + dz * dz;

        // If discrepancy exceeds snap threshold (e.g. teleport, respawn), snap instantly
        if (dist_sq > m_config.snap_distance_threshold * m_config.snap_distance_threshold) {
            reset(new_snapshot);
            return;
        }

        m_current = m_target;
        m_target = new_snapshot;
        m_time_since_target = 0.0f;
    }

    /**
     * Adds client prediction error offsets to decay smoothly over time.
     */
    void add_prediction_error(float err_x, float err_y, float err_z) {
        m_error_x += err_x;
        m_error_y += err_y;
        m_error_z += err_z;
    }

    /**
     * Evaluates the interpolated/extrapolated transform at the current frame.
     */
    TransformSnapshot evaluate(float delta_time) {
        if (!m_initialized) {
            return TransformSnapshot{};
        }

        m_time_since_target += delta_time;

        TransformSnapshot result;

        // Exponential error decay
        if (m_config.error_decay_half_life > 0.0001f && delta_time > 0.0f) {
            float decay = std::exp(-delta_time * (0.693147f / m_config.error_decay_half_life));
            m_error_x *= decay;
            m_error_y *= decay;
            m_error_z *= decay;
        } else if (delta_time > 0.0f) {
            m_error_x = 0.0f;
            m_error_y = 0.0f;
            m_error_z = 0.0f;
        }

        if (m_time_since_target <= m_config.interpolation_delay_seconds) {
            // Hermite interpolation between current and target
            float t = m_config.interpolation_delay_seconds > 0.0001f
                ? std::clamp(m_time_since_target / m_config.interpolation_delay_seconds, 0.0f, 1.0f)
                : 1.0f;

            // Smoothstep blending: 3t^2 - 2t^3
            float blend = t * t * (3.0f - 2.0f * t);

            result.x = m_current.x + (m_target.x - m_current.x) * blend + m_error_x;
            result.y = m_current.y + (m_target.y - m_current.y) * blend + m_error_y;
            result.z = m_current.z + (m_target.z - m_current.z) * blend + m_error_z;
            result.yaw_degrees = interpolate_angle(m_current.yaw_degrees, m_target.yaw_degrees, blend);
            result.velocity_x = m_current.velocity_x + (m_target.velocity_x - m_current.velocity_x) * blend;
            result.velocity_z = m_current.velocity_z + (m_target.velocity_z - m_current.velocity_z) * blend;
        } else {
            // Forward dead-reckoning extrapolation
            float extra_time = std::min(
                m_time_since_target - m_config.interpolation_delay_seconds,
                m_config.max_extrapolation_seconds
            );

            result.x = m_target.x + m_target.velocity_x * extra_time + m_error_x;
            result.y = m_target.y + m_error_y;
            result.z = m_target.z + m_target.velocity_z * extra_time + m_error_z;
            result.yaw_degrees = m_target.yaw_degrees;
            result.velocity_x = m_target.velocity_x;
            result.velocity_z = m_target.velocity_z;
        }

        result.timestamp = m_target.timestamp + m_time_since_target;
        return result;
    }

    /**
     * Checks whether the interpolator has received at least one valid snapshot.
     */
    bool is_initialized() const {
        return m_initialized;
    }

    /**
     * Returns the active configuration.
     */
    const InterpolatorConfig& config() const {
        return m_config;
    }

    /**
     * Updates the active configuration.
     */
    void set_config(const InterpolatorConfig& config) {
        m_config = config;
    }

private:
    static float interpolate_angle(float from_deg, float to_deg, float t) {
        float diff = std::fmod(to_deg - from_deg, 360.0f);
        if (diff > 180.0f) {
            diff -= 360.0f;
        } else if (diff < -180.0f) {
            diff += 360.0f;
        }
        return from_deg + diff * t;
    }

    InterpolatorConfig m_config;
    TransformSnapshot m_current;
    TransformSnapshot m_target;
    float m_error_x = 0.0f;
    float m_error_y = 0.0f;
    float m_error_z = 0.0f;
    float m_time_since_target = 0.0f;
    bool m_initialized = false;
};

} // namespace eidolon

#endif // EIDOLON_INTERPOLATOR_HPP
