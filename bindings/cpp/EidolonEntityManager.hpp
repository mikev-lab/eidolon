// EidolonEntityManager.hpp: High-Level Client Entity Registry & Lifecycle Manager
// Manages Area of Interest (AoI) entity states, dead reckoning interpolators, and lifecycle callbacks.
// Strictly standard C++17 with zero third-party dependencies.

#ifndef EIDOLON_ENTITY_MANAGER_HPP
#define EIDOLON_ENTITY_MANAGER_HPP

#include "EidolonInterpolator.hpp"

#include <cstdint>
#include <functional>
#include <optional>
#include <unordered_map>
#include <vector>

namespace eidolon {

/**
 * High-level state tracked per visible entity.
 */
struct EntityState {
    uint32_t entity_id = 0;
    uint8_t entity_type = 0;
    Interpolator interpolator;
    bool active = true;
    double last_update_time = 0.0;
};

/**
 * High-level entity registry coordinating client-side entities and interpolation.
 */
class EntityManager {
public:
    using SpawnCallback = std::function<void(uint32_t entity_id, uint8_t entity_type, const TransformSnapshot& initial)>;
    using DespawnCallback = std::function<void(uint32_t entity_id)>;
    using UpdateCallback = std::function<void(uint32_t entity_id, const TransformSnapshot& interpolated)>;

    explicit EntityManager(const InterpolatorConfig& config = InterpolatorConfig{})
        : m_default_config(config)
    {
    }

    /**
     * Registers a new entity spawned into the client's Area of Interest.
     */
    void on_entity_spawned(uint32_t entity_id, uint8_t entity_type, const TransformSnapshot& initial) {
        EntityState state;
        state.entity_id = entity_id;
        state.entity_type = entity_type;
        state.interpolator = Interpolator(m_default_config);
        state.interpolator.reset(initial);
        state.active = true;
        state.last_update_time = initial.timestamp;

        m_entities[entity_id] = std::move(state);

        if (m_on_spawn) {
            m_on_spawn(entity_id, entity_type, initial);
        }
    }

    /**
     * Updates an existing entity with an authoritative transform update.
     */
    void on_entity_updated(uint32_t entity_id, const TransformSnapshot& snapshot) {
        auto it = m_entities.find(entity_id);
        if (it != m_entities.end()) {
            it->second.interpolator.update_target(snapshot);
            it->second.last_update_time = snapshot.timestamp;
        } else {
            // Auto-spawn if not previously registered
            on_entity_spawned(entity_id, 0, snapshot);
        }
    }

    /**
     * Removes an entity that has despawned or exited the client's Area of Interest.
     */
    void on_entity_despawned(uint32_t entity_id) {
        auto it = m_entities.find(entity_id);
        if (it != m_entities.end()) {
            m_entities.erase(it);
            if (m_on_despawn) {
                m_on_despawn(entity_id);
            }
        }
    }

    /**
     * Evaluates the visual transform of an entity for rendering at the current frame.
     */
    std::optional<TransformSnapshot> evaluate_entity(uint32_t entity_id, float delta_time) {
        auto it = m_entities.find(entity_id);
        if (it == m_entities.end() || !it->second.active) {
            return std::nullopt;
        }

        TransformSnapshot snapshot = it->second.interpolator.evaluate(delta_time);

        if (m_on_update) {
            m_on_update(entity_id, snapshot);
        }

        return snapshot;
    }

    /**
     * Evaluates all active entities, invoking the update callback for each.
     */
    void evaluate_all(float delta_time) {
        for (auto& pair : m_entities) {
            if (pair.second.active) {
                TransformSnapshot snapshot = pair.second.interpolator.evaluate(delta_time);
                if (m_on_update) {
                    m_on_update(pair.first, snapshot);
                }
            }
        }
    }

    /**
     * Checks if an entity is currently registered in the registry.
     */
    bool has_entity(uint32_t entity_id) const {
        return m_entities.find(entity_id) != m_entities.end();
    }

    /**
     * Returns the total count of active entities.
     */
    size_t count() const {
        return m_entities.size();
    }

    /**
     * Returns a list of all currently tracked entity IDs.
     */
    std::vector<uint32_t> get_entity_ids() const {
        std::vector<uint32_t> ids;
        ids.reserve(m_entities.size());
        for (const auto& pair : m_entities) {
            ids.push_back(pair.first);
        }
        return ids;
    }

    /**
     * Clears all entities from the registry (e.g. on disconnect or zone transition).
     */
    void clear() {
        if (m_on_despawn) {
            for (const auto& pair : m_entities) {
                m_on_despawn(pair.first);
            }
        }
        m_entities.clear();
    }

    /**
     * Registers a callback invoked when an entity spawns into Area of Interest.
     */
    void set_spawn_callback(SpawnCallback cb) {
        m_on_spawn = std::move(cb);
    }

    /**
     * Registers a callback invoked when an entity despawns or leaves Area of Interest.
     */
    void set_despawn_callback(DespawnCallback cb) {
        m_on_despawn = std::move(cb);
    }

    /**
     * Registers a callback invoked per frame with interpolated visual transforms.
     */
    void set_update_callback(UpdateCallback cb) {
        m_on_update = std::move(cb);
    }

private:
    InterpolatorConfig m_default_config;
    std::unordered_map<uint32_t, EntityState> m_entities;
    SpawnCallback m_on_spawn;
    DespawnCallback m_on_despawn;
    UpdateCallback m_on_update;
};

} // namespace eidolon

#endif // EIDOLON_ENTITY_MANAGER_HPP
