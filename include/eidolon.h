/*
 * eidolon.h: Universal C ABI Header for Eidolon MMO Engine Client
 *
 * Compatible with C99, C++, Unreal Engine 5, Godot GDExtension, and custom engines.
 * Zero external dependencies beyond standard C headers.
 */

#ifndef EIDOLON_H
#define EIDOLON_H

#include <stdint.h>
#include <stdbool.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ========================================================================= */
/* Status and Error Codes                                                    */
/* ========================================================================= */

#define EIDOLON_OK                     0
#define EIDOLON_ERR_NULL_PTR         (-1)
#define EIDOLON_ERR_INVALID_HOST     (-2)
#define EIDOLON_ERR_NETWORK          (-3)
#define EIDOLON_ERR_NOT_CONNECTED    (-4)
#define EIDOLON_ERR_PANIC            (-5)
#define EIDOLON_ERR_ENTITY_NOT_FOUND (-6)

/* ========================================================================= */
/* Event Classifications                                                     */
/* ========================================================================= */

#define EIDOLON_EVENT_CONNECTED       1
#define EIDOLON_EVENT_DISCONNECTED    2
#define EIDOLON_EVENT_ENTITY_SPAWNED  3
#define EIDOLON_EVENT_ENTITY_DESPAWNED 4
#define EIDOLON_EVENT_ENTITY_UPDATED  5
#define EIDOLON_EVENT_COMBAT_ACTION   6
#define EIDOLON_EVENT_LOOT_ACQUIRED   7
#define EIDOLON_EVENT_CAST_STARTED    8
#define EIDOLON_EVENT_CAST_INTERRUPTED 9
#define EIDOLON_EVENT_CAST_COMPLETED 10
#define EIDOLON_EVENT_CHAT_MESSAGE   11
#define EIDOLON_EVENT_EQUIPMENT_CHANGED 12
#define EIDOLON_EVENT_PARTY_UPDATED  13
#define EIDOLON_EVENT_TIME_DILATION_CHANGED 14

/* ========================================================================= */
/* Density Profiles                                                          */
/* ========================================================================= */

#define EIDOLON_DENSITY_BUDGET_MOBILE         0
#define EIDOLON_DENSITY_STANDARD_MMO          1
#define EIDOLON_DENSITY_MASSIVE_FLEET_OR_SIEGE 2

/* ========================================================================= */
/* Data Structures                                                           */
/* ========================================================================= */

/**
 * Standard continuous transform representation for rendering engines.
 */
typedef struct EidolonTransform {
    float x;
    float y;
    float z;
    float yaw_degrees;
    float velocity_x;
    float velocity_z;
    uint8_t flags;
    uint8_t _padding[3];
} EidolonTransform;

/**
 * Event notification dispatched from client network loop to game engine.
 */
typedef struct EidolonEvent {
    uint32_t event_type;
    uint32_t entity_id;
    float x;
    float y;
    float z;
    float yaw_degrees;
    uint64_t param1;
    uint64_t param2;
} EidolonEvent;

/**
 * Opaque handle representing an active EidolonClient instance.
 */
typedef struct EidolonClientHandle EidolonClientHandle;

/**
 * Function pointer signature for receiving polled events.
 */
typedef void (*EidolonEventCallback)(const EidolonEvent* event, void* user_data);

/* ========================================================================= */
/* Client Lifecycle API                                                      */
/* ========================================================================= */

/**
 * Allocates and initializes a new unmanaged EidolonClient handle.
 *
 * Returns NULL if allocation fails.
 */
EidolonClientHandle* eidolon_client_create(void);

/**
 * Gracefully disconnects and destroys an EidolonClient handle, freeing all resources.
 */
void eidolon_client_destroy(EidolonClientHandle* handle);

/**
 * Connects to an authoritative eidolon-server and initiates cryptographic challenge handshake.
 *
 * @param handle Valid client handle.
 * @param host Null-terminated string (e.g. "127.0.0.1" or "mmo.example.com").
 * @param port Server UDP port (e.g. 7777).
 * @param account_id Unique 64-bit player account identifier.
 * @param ticket_high High 64 bits of the 128-bit session ticket.
 * @param ticket_low Low 64 bits of the 128-bit session ticket.
 * @return EIDOLON_OK on success, or negative error code.
 */
int32_t eidolon_client_connect(
    EidolonClientHandle* handle,
    const char* host,
    uint16_t port,
    uint64_t account_id,
    uint64_t ticket_high,
    uint64_t ticket_low
);

/**
 * Gracefully disconnects from the server and clears the local world view.
 */
void eidolon_client_disconnect(EidolonClientHandle* handle);

/**
 * Drains incoming UDP packets and dispatches queued events to the callback function.
 *
 * @param handle Valid client handle.
 * @param callback Function pointer invoked for each event.
 * @param user_data Arbitrary pointer forwarded to callback.
 * @return Number of events processed.
 */
uint32_t eidolon_client_poll_events(
    EidolonClientHandle* handle,
    EidolonEventCallback callback,
    void* user_data
);

/**
 * Dispatches continuous player movement intent to the server.
 *
 * @param handle Valid client handle.
 * @param move_x Velocity along X axis in meters/second.
 * @param move_z Velocity along Z axis in meters/second.
 * @param yaw_deg Facing angle in degrees (0.0 to 360.0).
 * @param flags Movement state flags (1 = walking, 2 = sprinting, 4 = jumping).
 * @return EIDOLON_OK on success, or negative error code.
 */
int32_t eidolon_client_send_intent(
    EidolonClientHandle* handle,
    float move_x,
    float move_z,
    float yaw_deg,
    uint8_t flags
);

/**
 * Dispatches a reliable gameplay action (attack, ability, interact).
 *
 * @param handle Valid client handle.
 * @param action_type Action identifier (1 = attack, 2 = interact, 3 = loot).
 * @param target_id Authoritative target entity ID.
 * @param param Action parameter (e.g. skill ID, slot index).
 * @return EIDOLON_OK on success, or negative error code.
 */
int32_t eidolon_client_send_action(
    EidolonClientHandle* handle,
    uint8_t action_type,
    uint32_t target_id,
    uint32_t param
);

/**
 * Dispatches an authoritative ability cast command.
 *
 * @param handle Valid client handle.
 * @param target_id Target entity identifier (or 0 for ground/self).
 * @param ability_id Blueprint ability identifier.
 * @return EIDOLON_OK on success, or negative error code.
 */
int32_t eidolon_client_cast_ability(
    EidolonClientHandle* handle,
    uint32_t target_id,
    uint32_t ability_id
);

/**
 * Dispatches an item equip command.
 *
 * @param handle Valid client handle.
 * @param inventory_slot Bag inventory slot index.
 * @param equip_slot Equipment slot identifier (0..8).
 * @return EIDOLON_OK on success, or negative error code.
 */
int32_t eidolon_client_equip_item(
    EidolonClientHandle* handle,
    uint8_t inventory_slot,
    uint8_t equip_slot
);

/**
 * Dispatches an item unequip command.
 *
 * @param handle Valid client handle.
 * @param equip_slot Equipment slot identifier (0..8).
 * @return EIDOLON_OK on success, or negative error code.
 */
int32_t eidolon_client_unequip_item(
    EidolonClientHandle* handle,
    uint8_t equip_slot
);

/**
 * Dispatches a multi-channel chat message.
 *
 * @param handle Valid client handle.
 * @param channel Channel scope (0 = Proximity, 1 = Party, 2 = Whisper, 3 = Global).
 * @param target_id Target entity ID for direct whispers (or 0).
 * @param text Null-terminated UTF-8 text string.
 * @return EIDOLON_OK on success, or negative error code.
 */
int32_t eidolon_client_send_chat(
    EidolonClientHandle* handle,
    uint8_t channel,
    uint32_t target_id,
    const char* text
);

/**
 * Dispatches a party management command (1 = invite, 2 = accept, 3 = leave).
 *
 * @param handle Valid client handle.
 * @param cmd Party command opcode.
 * @param target_account Target account identifier.
 * @return EIDOLON_OK on success, or negative error code.
 */
int32_t eidolon_client_party_command(
    EidolonClientHandle* handle,
    uint8_t cmd,
    uint64_t target_account
);

/**
 * Extrapolates an entity's transform to the current render frame using deterministic dead reckoning.
 *
 * @param handle Valid client handle.
 * @param entity_id Entity to extrapolate.
 * @param delta_time Elapsed seconds since last frame (e.g. 0.0166 for 60 FPS).
 * @param out_transform Pointer to EidolonTransform structure to populate.
 * @return EIDOLON_OK on success, or EIDOLON_ERR_ENTITY_NOT_FOUND.
 */
int32_t eidolon_client_extrapolate_entity(
    EidolonClientHandle* handle,
    uint32_t entity_id,
    float delta_time,
    EidolonTransform* out_transform
);

/**
 * Returns the total number of visible entities in the client's Area of Interest.
 */
uint32_t eidolon_client_get_visible_entity_count(EidolonClientHandle* handle);

/**
 * Copies up to max_count visible entity IDs into out_buffer.
 *
 * @return Number of entity IDs written.
 */
uint32_t eidolon_client_get_visible_entities(
    EidolonClientHandle* handle,
    uint32_t* out_buffer,
    uint32_t max_count
);

/**
 * Checks whether the client has completed cryptographic authentication and is connected.
 *
 * @return 1 if connected, 0 otherwise.
 */
int32_t eidolon_client_is_connected(EidolonClientHandle* handle);

/**
 * Sets the client density profile preference (0 = Mobile, 1 = Standard MMO, 2 = Massive Fleet/Siege).
 *
 * @param handle Valid client handle.
 * @param profile Density profile identifier.
 * @return EIDOLON_OK on success, or negative error code.
 */
int32_t eidolon_client_set_density_profile(
    EidolonClientHandle* handle,
    uint32_t profile
);

/**
 * Returns the current client density profile setting.
 *
 * @param handle Valid client handle.
 * @return Density profile identifier.
 */
uint32_t eidolon_client_get_density_profile(EidolonClientHandle* handle);

/**
 * Returns the active server-mandated Time Dilation (TiDi) factor (1.0 = full speed, <1.0 = dilated).
 *
 * @param handle Valid client handle.
 * @return Time dilation scale factor.
 */
float eidolon_client_get_time_dilation(EidolonClientHandle* handle);

/**
 * Explicitly sets the client-side time dilation factor.
 *
 * @param handle Valid client handle.
 * @param time_dilation Desired time dilation factor.
 * @return EIDOLON_OK on success, or negative error code.
 */
int32_t eidolon_client_set_time_dilation(
    EidolonClientHandle* handle,
    float time_dilation
);

#ifdef __cplusplus
}
#endif

#endif /* EIDOLON_H */
