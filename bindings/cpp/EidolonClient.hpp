// EidolonClient.hpp: C++17 RAII Wrapper for Eidolon MMO Engine Client
// Compatible with Unreal Engine 5, custom engines, and modern C++ applications.

#ifndef EIDOLON_CLIENT_HPP
#define EIDOLON_CLIENT_HPP

#include "eidolon.h"

#include <functional>
#include <memory>
#include <optional>
#include <stdexcept>
#include <string>
#include <vector>

namespace eidolon {

/**
 * Modern C++17 RAII client wrapper managing the lifecycle of an Eidolon client instance.
 */
class Client {
public:
    Client()
        : m_handle(eidolon_client_create(), eidolon_client_destroy)
    {
        if (!m_handle) {
            throw std::runtime_error("Failed to allocate EidolonClientHandle.");
        }
    }

    ~Client() = default;

    // Non-copyable
    Client(const Client&) = delete;
    Client& operator=(const Client&) = delete;

    // Movable
    Client(Client&&) noexcept = default;
    Client& operator=(Client&&) noexcept = default;

    /**
     * Connects to the authoritative server and completes the cryptographic challenge.
     */
    bool connect(
        const std::string& host,
        uint16_t port,
        uint64_t account_id,
        uint64_t ticket_high,
        uint64_t ticket_low)
    {
        int32_t res = eidolon_client_connect(
            m_handle.get(),
            host.c_str(),
            port,
            account_id,
            ticket_high,
            ticket_low
        );
        return res == EIDOLON_OK;
    }

    /**
     * Gracefully disconnects from the server.
     */
    void disconnect() {
        eidolon_client_disconnect(m_handle.get());
    }

    /**
     * Drains incoming packets and invokes the callback for each event.
     */
    uint32_t poll_events(const std::function<void(const EidolonEvent&)>& callback) {
        auto cb_wrapper = [](const EidolonEvent* evt, void* user_data) {
            auto* cb = static_cast<const std::function<void(const EidolonEvent&)>*>(user_data);
            if (cb && evt) {
                (*cb)(*evt);
            }
        };

        return eidolon_client_poll_events(
            m_handle.get(),
            cb_wrapper,
            const_cast<void*>(static_cast<const void*>(&callback))
        );
    }

    /**
     * Dispatches continuous movement intent.
     */
    bool send_movement(float move_x, float move_z, float yaw_deg, uint8_t flags = 0) {
        return eidolon_client_send_intent(m_handle.get(), move_x, move_z, yaw_deg, flags) == EIDOLON_OK;
    }

    /**
     * Dispatches a reliable gameplay action.
     */
    bool send_action(uint8_t action_type, uint32_t target_id, uint32_t param = 0) {
        return eidolon_client_send_action(m_handle.get(), action_type, target_id, param) == EIDOLON_OK;
    }

    /**
     * Dispatches an authoritative ability cast command.
     */
    bool cast_ability(uint32_t target_id, uint32_t ability_id) {
        return eidolon_client_cast_ability(m_handle.get(), target_id, ability_id) == EIDOLON_OK;
    }

    /**
     * Dispatches an item equip command.
     */
    bool equip_item(uint8_t inventory_slot, uint8_t equip_slot) {
        return eidolon_client_equip_item(m_handle.get(), inventory_slot, equip_slot) == EIDOLON_OK;
    }

    /**
     * Dispatches an item unequip command.
     */
    bool unequip_item(uint8_t equip_slot) {
        return eidolon_client_unequip_item(m_handle.get(), equip_slot) == EIDOLON_OK;
    }

    /**
     * Dispatches a multi-channel chat message.
     */
    bool send_chat(uint8_t channel, uint32_t target_id, const std::string& text) {
        return eidolon_client_send_chat(m_handle.get(), channel, target_id, text.c_str()) == EIDOLON_OK;
    }

    /**
     * Dispatches a party management command (1 = invite, 2 = accept, 3 = leave).
     */
    bool party_command(uint8_t cmd, uint64_t target_account) {
        return eidolon_client_party_command(m_handle.get(), cmd, target_account) == EIDOLON_OK;
    }

    /**
     * Extrapolates an entity's transform using deterministic dead reckoning.
     */
    std::optional<EidolonTransform> extrapolate_entity(uint32_t entity_id, float delta_time) {
        EidolonTransform out_tf{};
        int32_t res = eidolon_client_extrapolate_entity(m_handle.get(), entity_id, delta_time, &out_tf);
        if (res == EIDOLON_OK) {
            return out_tf;
        }
        return std::nullopt;
    }

    /**
     * Returns a list of all visible entity IDs currently tracked in Area of Interest.
     */
    std::vector<uint32_t> get_visible_entities() {
        uint32_t count = eidolon_client_get_visible_entity_count(m_handle.get());
        if (count == 0) {
            return {};
        }

        std::vector<uint32_t> ids(count);
        uint32_t written = eidolon_client_get_visible_entities(m_handle.get(), ids.data(), count);
        ids.resize(written);
        return ids;
    }

    /**
     * Checks if the client has established an authenticated connection.
     */
    bool is_connected() const {
        return eidolon_client_is_connected(m_handle.get()) == 1;
    }

    /**
     * Sets the client density profile preference (0 = Mobile, 1 = Standard MMO, 2 = Massive Fleet/Siege).
     */
    bool set_density_profile(uint32_t profile) {
        return eidolon_client_set_density_profile(m_handle.get(), profile) == EIDOLON_OK;
    }

    /**
     * Returns the current client density profile setting.
     */
    uint32_t get_density_profile() const {
        return eidolon_client_get_density_profile(m_handle.get());
    }

    /**
     * Returns the active server-mandated Time Dilation (TiDi) factor (1.0 = full speed, <1.0 = dilated).
     */
    float get_time_dilation() const {
        return eidolon_client_get_time_dilation(m_handle.get());
    }

    /**
     * Explicitly sets the client-side time dilation factor.
     */
    bool set_time_dilation(float time_dilation) {
        return eidolon_client_set_time_dilation(m_handle.get(), time_dilation) == EIDOLON_OK;
    }

    /**
     * Dispatches a building placement request (modular prefab or freeform piece).
     */
    bool place_structure(uint32_t prefab_type_id, float x, float y, float z, float yaw_degrees) {
        return eidolon_client_place_structure(m_handle.get(), prefab_type_id, x, y, z, yaw_degrees) == EIDOLON_OK;
    }

    /**
     * Dispatches a structure demolition request.
     */
    bool destroy_structure(uint32_t structure_id) {
        return eidolon_client_destroy_structure(m_handle.get(), structure_id) == EIDOLON_OK;
    }

    /**
     * Dispatches an interior cell entry request.
     */
    bool enter_interior_cell(uint32_t cell_id) {
        return eidolon_client_enter_interior_cell(m_handle.get(), cell_id) == EIDOLON_OK;
    }

    /**
     * Dispatches an interior cell exit request back to open-world space.
     */
    bool exit_interior_cell() {
        return eidolon_client_exit_interior_cell(m_handle.get()) == EIDOLON_OK;
    }

    /**
     * Dispatches a decorative interior item move or placement command.
     */
    bool move_interior_item(uint32_t cell_id, uint32_t item_instance_id, float local_x, float local_y, float local_z, float yaw_degrees) {
        return eidolon_client_move_interior_item(m_handle.get(), cell_id, item_instance_id, local_x, local_y, local_z, yaw_degrees) == EIDOLON_OK;
    }

    /**
     * Returns the raw unmanaged handle for custom extensions.
     */
    EidolonClientHandle* raw_handle() const {
        return m_handle.get();
    }

private:
    std::unique_ptr<EidolonClientHandle, decltype(&eidolon_client_destroy)> m_handle;
};

} // namespace eidolon

#endif // EIDOLON_CLIENT_HPP
