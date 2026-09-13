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

#ifdef __cplusplus
}
#endif

#endif /* EIDOLON_H */
