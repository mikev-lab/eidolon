//! C ABI exports for dynamic linking and cross-language game engine integration.

use std::ffi::{c_char, c_void, CStr};
use std::net::{SocketAddr, ToSocketAddrs};
use std::panic::catch_unwind;

use eidolon_client::{ClientConfig, ClientEvent, EidolonClient};
use eidolon_core::identity::{AccountId, SessionTicket};

use crate::types::{
    EidolonEvent, EidolonGlobalCoord, EidolonTransform, EidolonVehicleIntent,
    EIDOLON_DENSITY_STANDARD_MMO, EIDOLON_EVENT_CAST_COMPLETED, EIDOLON_EVENT_CAST_INTERRUPTED,
    EIDOLON_EVENT_CAST_STARTED, EIDOLON_EVENT_CHAT_MESSAGE, EIDOLON_EVENT_COMBAT_ACTION,
    EIDOLON_EVENT_CONNECTED, EIDOLON_EVENT_DISCONNECTED, EIDOLON_EVENT_ENTITY_DESPAWNED,
    EIDOLON_EVENT_ENTITY_SPAWNED, EIDOLON_EVENT_ENTITY_UPDATED, EIDOLON_EVENT_EQUIPMENT_CHANGED,
    EIDOLON_EVENT_LOOT_ACQUIRED, EIDOLON_EVENT_PARTY_UPDATED, EIDOLON_EVENT_TIME_DILATION_CHANGED,
};

/// Opaque handle representing an `EidolonClient` instance across the C ABI.
pub struct EidolonClientHandle {
    client: Option<EidolonClient>,
    density_profile: u32,
    time_dilation: f32,
}

/// Callback function signature invoked for each polled event.
pub type EidolonEventCallback =
    Option<unsafe extern "C" fn(event: *const EidolonEvent, user_data: *mut c_void)>;

/// Success return code.
pub const EIDOLON_OK: i32 = 0;
/// Error return code: Null pointer provided to FFI function.
pub const EIDOLON_ERR_NULL_PTR: i32 = -1;
/// Error return code: Host string was invalid UTF-8 or could not be resolved.
pub const EIDOLON_ERR_INVALID_HOST: i32 = -2;
/// Error return code: I/O or network failure.
pub const EIDOLON_ERR_NETWORK: i32 = -3;
/// Error return code: Client is not in connected state.
pub const EIDOLON_ERR_NOT_CONNECTED: i32 = -4;
/// Error return code: Internal panic caught at FFI boundary.
pub const EIDOLON_ERR_PANIC: i32 = -5;
/// Error return code: Entity was not found in client Area of Interest.
pub const EIDOLON_ERR_ENTITY_NOT_FOUND: i32 = -6;

/// Allocates and initializes a new unmanaged `EidolonClient` handle.
///
/// Returns null if memory allocation fails or internal panic occurs.
#[no_mangle]
pub extern "C" fn eidolon_client_create() -> *mut EidolonClientHandle {
    let result = catch_unwind(|| {
        let handle = Box::new(EidolonClientHandle {
            client: None,
            density_profile: EIDOLON_DENSITY_STANDARD_MMO,
            time_dilation: 1.0,
        });
        Box::into_raw(handle)
    });

    result.unwrap_or(std::ptr::null_mut())
}

/// Destroys an `EidolonClient` handle, disconnecting if necessary and releasing all resources.
#[no_mangle]
pub extern "C" fn eidolon_client_destroy(handle: *mut EidolonClientHandle) {
    if handle.is_null() {
        return;
    }

    let _ = catch_unwind(|| {
        // SAFETY: Pointer is confirmed non-null above and was allocated via `Box::into_raw` in `eidolon_client_create`.
        let mut boxed = unsafe { Box::from_raw(handle) };
        if let Some(ref mut client) = boxed.client {
            client.disconnect();
        }
    });
}

/// Connects to an authoritative `eidolon-server` and initiates cryptographic handshake.
#[no_mangle]
pub extern "C" fn eidolon_client_connect(
    handle: *mut EidolonClientHandle,
    host: *const c_char,
    port: u16,
    account_id: u64,
    ticket_high: u64,
    ticket_low: u64,
) -> i32 {
    if handle.is_null() || host.is_null() {
        return EIDOLON_ERR_NULL_PTR;
    }

    let result = catch_unwind(|| {
        // SAFETY: `host` is non-null and assumed to be a null-terminated C string provided by the caller.
        let host_str = match unsafe { CStr::from_ptr(host) }.to_str() {
            Ok(s) => s,
            Err(_) => return EIDOLON_ERR_INVALID_HOST,
        };

        let addr_str = format!("{host_str}:{port}");
        let socket_addr: SocketAddr = match addr_str.to_socket_addrs() {
            Ok(mut addrs) => match addrs.next() {
                Some(a) => a,
                None => return EIDOLON_ERR_INVALID_HOST,
            },
            Err(_) => return EIDOLON_ERR_INVALID_HOST,
        };

        let mut ticket_bytes = [0u8; 16];
        ticket_bytes[..8].copy_from_slice(&ticket_high.to_be_bytes());
        ticket_bytes[8..16].copy_from_slice(&ticket_low.to_be_bytes());

        let config = match ClientConfig::new(
            socket_addr,
            AccountId(account_id),
            SessionTicket(ticket_bytes),
        ) {
            Ok(cfg) => cfg,
            Err(_) => return EIDOLON_ERR_NETWORK,
        };

        let mut client = match EidolonClient::new(config) {
            Ok(c) => c,
            Err(_) => return EIDOLON_ERR_NETWORK,
        };

        if client.connect().is_err() {
            return EIDOLON_ERR_NETWORK;
        }

        // SAFETY: `handle` was verified non-null above and points to a valid `EidolonClientHandle`.
        unsafe {
            (*handle).client = Some(client);
        }

        EIDOLON_OK
    });

    result.unwrap_or(EIDOLON_ERR_PANIC)
}

/// Gracefully disconnects the client from the server.
#[no_mangle]
pub extern "C" fn eidolon_client_disconnect(handle: *mut EidolonClientHandle) {
    if handle.is_null() {
        return;
    }

    let _ = catch_unwind(|| {
        // SAFETY: `handle` was verified non-null above.
        let handle_ref = unsafe { &mut *handle };
        if let Some(ref mut client) = handle_ref.client {
            client.disconnect();
        }
    });
}

/// Polls network events and invokes the provided callback for each event.
///
/// Returns the total number of events dispatched.
#[no_mangle]
pub extern "C" fn eidolon_client_poll_events(
    handle: *mut EidolonClientHandle,
    callback: EidolonEventCallback,
    user_data: *mut c_void,
) -> u32 {
    if handle.is_null() {
        return 0;
    }

    let result = catch_unwind(|| {
        // SAFETY: `handle` is non-null and points to a valid `EidolonClientHandle`.
        let handle_ref = unsafe { &mut *handle };
        let client = match handle_ref.client.as_mut() {
            Some(c) => c,
            None => return 0,
        };

        let events = match client.poll_events() {
            Ok(evts) => evts,
            Err(_) => return 0,
        };

        let count = events.len() as u32;

        if let Some(cb) = callback {
            for event in events {
                let c_event = match event {
                    ClientEvent::Connected {
                        server_version,
                        session_id,
                    } => EidolonEvent {
                        event_type: EIDOLON_EVENT_CONNECTED,
                        entity_id: 0,
                        x: 0.0,
                        y: 0.0,
                        z: 0.0,
                        yaw_degrees: 0.0,
                        param1: session_id,
                        param2: server_version as u64,
                    },
                    ClientEvent::Disconnected { .. } => EidolonEvent {
                        event_type: EIDOLON_EVENT_DISCONNECTED,
                        entity_id: 0,
                        x: 0.0,
                        y: 0.0,
                        z: 0.0,
                        yaw_degrees: 0.0,
                        param1: 0,
                        param2: 0,
                    },
                    ClientEvent::EntitySpawned {
                        entity_id,
                        entity_type,
                        x,
                        y,
                        z,
                        yaw_deg,
                    } => EidolonEvent {
                        event_type: EIDOLON_EVENT_ENTITY_SPAWNED,
                        entity_id,
                        x,
                        y,
                        z,
                        yaw_degrees: yaw_deg,
                        param1: entity_type as u64,
                        param2: 0,
                    },
                    ClientEvent::EntityDespawned { entity_id } => EidolonEvent {
                        event_type: EIDOLON_EVENT_ENTITY_DESPAWNED,
                        entity_id,
                        x: 0.0,
                        y: 0.0,
                        z: 0.0,
                        yaw_degrees: 0.0,
                        param1: 0,
                        param2: 0,
                    },
                    ClientEvent::EntityUpdated {
                        entity_id,
                        x,
                        y,
                        z,
                        yaw_deg,
                        flags,
                    } => EidolonEvent {
                        event_type: EIDOLON_EVENT_ENTITY_UPDATED,
                        entity_id,
                        x,
                        y,
                        z,
                        yaw_degrees: yaw_deg,
                        param1: flags as u64,
                        param2: 0,
                    },
                    ClientEvent::CombatAction {
                        source_id,
                        target_id,
                        action_type,
                        value,
                    } => EidolonEvent {
                        event_type: EIDOLON_EVENT_COMBAT_ACTION,
                        entity_id: source_id,
                        x: 0.0,
                        y: 0.0,
                        z: 0.0,
                        yaw_degrees: 0.0,
                        param1: target_id as u64,
                        param2: ((action_type as u64) << 32) | (value as u64),
                    },
                    ClientEvent::LootAcquired {
                        entity_id,
                        item_id,
                        amount,
                    } => EidolonEvent {
                        event_type: EIDOLON_EVENT_LOOT_ACQUIRED,
                        entity_id,
                        x: 0.0,
                        y: 0.0,
                        z: 0.0,
                        yaw_degrees: 0.0,
                        param1: item_id as u64,
                        param2: amount as u64,
                    },
                    ClientEvent::StateReconciled { position, yaw_deg } => EidolonEvent {
                        event_type: EIDOLON_EVENT_ENTITY_UPDATED,
                        entity_id: 0,
                        x: position[0],
                        y: position[1],
                        z: position[2],
                        yaw_degrees: yaw_deg,
                        param1: 0,
                        param2: 0,
                    },
                    ClientEvent::StateCorrection {
                        entity_id,
                        position,
                        yaw_deg,
                        ..
                    } => EidolonEvent {
                        event_type: EIDOLON_EVENT_ENTITY_UPDATED,
                        entity_id,
                        x: position[0],
                        y: position[1],
                        z: position[2],
                        yaw_degrees: yaw_deg,
                        param1: 0,
                        param2: 0,
                    },
                    ClientEvent::CastStarted {
                        entity_id,
                        ability_id,
                        duration_ticks,
                    } => EidolonEvent {
                        event_type: EIDOLON_EVENT_CAST_STARTED,
                        entity_id,
                        x: 0.0,
                        y: 0.0,
                        z: 0.0,
                        yaw_degrees: 0.0,
                        param1: ability_id as u64,
                        param2: duration_ticks as u64,
                    },
                    ClientEvent::CastInterrupted {
                        entity_id,
                        ability_id,
                        reason,
                    } => EidolonEvent {
                        event_type: EIDOLON_EVENT_CAST_INTERRUPTED,
                        entity_id,
                        x: 0.0,
                        y: 0.0,
                        z: 0.0,
                        yaw_degrees: 0.0,
                        param1: ability_id as u64,
                        param2: reason as u64,
                    },
                    ClientEvent::CastCompleted {
                        entity_id,
                        ability_id,
                    } => EidolonEvent {
                        event_type: EIDOLON_EVENT_CAST_COMPLETED,
                        entity_id,
                        x: 0.0,
                        y: 0.0,
                        z: 0.0,
                        yaw_degrees: 0.0,
                        param1: ability_id as u64,
                        param2: 0,
                    },
                    ClientEvent::ChatMessageReceived {
                        channel,
                        sender_id,
                        message: _,
                    } => EidolonEvent {
                        event_type: EIDOLON_EVENT_CHAT_MESSAGE,
                        entity_id: sender_id,
                        x: 0.0,
                        y: 0.0,
                        z: 0.0,
                        yaw_degrees: 0.0,
                        param1: channel as u64,
                        param2: 0,
                    },
                    ClientEvent::EquipmentChanged {
                        entity_id,
                        slot,
                        item_id,
                    } => EidolonEvent {
                        event_type: EIDOLON_EVENT_EQUIPMENT_CHANGED,
                        entity_id,
                        x: 0.0,
                        y: 0.0,
                        z: 0.0,
                        yaw_degrees: 0.0,
                        param1: slot as u64,
                        param2: item_id as u64,
                    },
                    ClientEvent::PartyUpdated {
                        party_id,
                        leader_account_id,
                        member_count,
                    } => EidolonEvent {
                        event_type: EIDOLON_EVENT_PARTY_UPDATED,
                        entity_id: 0,
                        x: 0.0,
                        y: 0.0,
                        z: 0.0,
                        yaw_degrees: 0.0,
                        param1: party_id,
                        param2: (leader_account_id << 8) | (member_count as u64),
                    },
                    ClientEvent::TimeDilationChanged { time_dilation } => {
                        handle_ref.time_dilation = time_dilation;
                        let factor_raw = (time_dilation as f64 * 4294967296.0) as u64;
                        EidolonEvent {
                            event_type: EIDOLON_EVENT_TIME_DILATION_CHANGED,
                            entity_id: 0,
                            x: 0.0,
                            y: 0.0,
                            z: 0.0,
                            yaw_degrees: 0.0,
                            param1: factor_raw,
                            param2: 0,
                        }
                    }
                };

                // SAFETY: `cb` is a valid C function pointer provided by the caller; `user_data` is passed through untouched.
                unsafe {
                    cb(&c_event, user_data);
                }
            }
        }

        count
    });

    result.unwrap_or(0)
}

/// Dispatches continuous player movement intent.
#[no_mangle]
pub extern "C" fn eidolon_client_send_intent(
    handle: *mut EidolonClientHandle,
    move_x: f32,
    move_z: f32,
    yaw_deg: f32,
    flags: u8,
) -> i32 {
    if handle.is_null() {
        return EIDOLON_ERR_NULL_PTR;
    }

    let result = catch_unwind(|| {
        // SAFETY: `handle` was verified non-null above.
        let handle_ref = unsafe { &mut *handle };
        let client = match handle_ref.client.as_mut() {
            Some(c) => c,
            None => return EIDOLON_ERR_NOT_CONNECTED,
        };

        match client.send_movement_intent(move_x, move_z, yaw_deg, flags) {
            Ok(_) => EIDOLON_OK,
            Err(_) => EIDOLON_ERR_NETWORK,
        }
    });

    result.unwrap_or(EIDOLON_ERR_PANIC)
}

/// Dispatches a reliable gameplay action (e.g. combat attack, item interaction).
#[no_mangle]
pub extern "C" fn eidolon_client_send_action(
    handle: *mut EidolonClientHandle,
    action_type: u8,
    target_id: u32,
    param: u32,
) -> i32 {
    if handle.is_null() {
        return EIDOLON_ERR_NULL_PTR;
    }

    let result = catch_unwind(|| {
        // SAFETY: `handle` was verified non-null above.
        let handle_ref = unsafe { &mut *handle };
        let client = match handle_ref.client.as_mut() {
            Some(c) => c,
            None => return EIDOLON_ERR_NOT_CONNECTED,
        };

        match client.send_action(action_type, target_id, param) {
            Ok(_) => EIDOLON_OK,
            Err(_) => EIDOLON_ERR_NETWORK,
        }
    });

    result.unwrap_or(EIDOLON_ERR_PANIC)
}

/// Dispatches an authoritative ability cast command.
#[no_mangle]
pub extern "C" fn eidolon_client_cast_ability(
    handle: *mut EidolonClientHandle,
    target_id: u32,
    ability_id: u32,
) -> i32 {
    if handle.is_null() {
        return EIDOLON_ERR_NULL_PTR;
    }

    let result = catch_unwind(|| {
        // SAFETY: `handle` was verified non-null above.
        let handle_ref = unsafe { &mut *handle };
        let client = match handle_ref.client.as_mut() {
            Some(c) => c,
            None => return EIDOLON_ERR_NOT_CONNECTED,
        };

        match client.cast_ability(target_id, ability_id) {
            Ok(_) => EIDOLON_OK,
            Err(_) => EIDOLON_ERR_NETWORK,
        }
    });

    result.unwrap_or(EIDOLON_ERR_PANIC)
}

/// Dispatches an item equip command.
#[no_mangle]
pub extern "C" fn eidolon_client_equip_item(
    handle: *mut EidolonClientHandle,
    inventory_slot: u8,
    equip_slot: u8,
) -> i32 {
    if handle.is_null() {
        return EIDOLON_ERR_NULL_PTR;
    }

    let result = catch_unwind(|| {
        // SAFETY: `handle` was verified non-null above.
        let handle_ref = unsafe { &mut *handle };
        let client = match handle_ref.client.as_mut() {
            Some(c) => c,
            None => return EIDOLON_ERR_NOT_CONNECTED,
        };

        match client.equip_item(inventory_slot, equip_slot) {
            Ok(_) => EIDOLON_OK,
            Err(_) => EIDOLON_ERR_NETWORK,
        }
    });

    result.unwrap_or(EIDOLON_ERR_PANIC)
}

/// Dispatches an item unequip command.
#[no_mangle]
pub extern "C" fn eidolon_client_unequip_item(
    handle: *mut EidolonClientHandle,
    equip_slot: u8,
) -> i32 {
    if handle.is_null() {
        return EIDOLON_ERR_NULL_PTR;
    }

    let result = catch_unwind(|| {
        // SAFETY: `handle` was verified non-null above.
        let handle_ref = unsafe { &mut *handle };
        let client = match handle_ref.client.as_mut() {
            Some(c) => c,
            None => return EIDOLON_ERR_NOT_CONNECTED,
        };

        match client.unequip_item(equip_slot) {
            Ok(_) => EIDOLON_OK,
            Err(_) => EIDOLON_ERR_NETWORK,
        }
    });

    result.unwrap_or(EIDOLON_ERR_PANIC)
}

/// Dispatches a multi-channel chat message.
#[no_mangle]
pub extern "C" fn eidolon_client_send_chat(
    handle: *mut EidolonClientHandle,
    channel: u8,
    target_id: u32,
    text: *const c_char,
) -> i32 {
    if handle.is_null() || text.is_null() {
        return EIDOLON_ERR_NULL_PTR;
    }

    let result = catch_unwind(|| {
        // SAFETY: `handle` and `text` are verified non-null above.
        let text_str = match unsafe { CStr::from_ptr(text) }.to_str() {
            Ok(s) => s,
            Err(_) => return EIDOLON_ERR_INVALID_HOST,
        };

        let handle_ref = unsafe { &mut *handle };
        let client = match handle_ref.client.as_mut() {
            Some(c) => c,
            None => return EIDOLON_ERR_NOT_CONNECTED,
        };

        match client.send_chat(channel, target_id, text_str) {
            Ok(_) => EIDOLON_OK,
            Err(_) => EIDOLON_ERR_NETWORK,
        }
    });

    result.unwrap_or(EIDOLON_ERR_PANIC)
}

/// Dispatches a party command (1 = invite, 2 = accept, 3 = leave).
#[no_mangle]
pub extern "C" fn eidolon_client_party_command(
    handle: *mut EidolonClientHandle,
    cmd: u8,
    target_account: u64,
) -> i32 {
    if handle.is_null() {
        return EIDOLON_ERR_NULL_PTR;
    }

    let result = catch_unwind(|| {
        // SAFETY: `handle` was verified non-null above.
        let handle_ref = unsafe { &mut *handle };
        let client = match handle_ref.client.as_mut() {
            Some(c) => c,
            None => return EIDOLON_ERR_NOT_CONNECTED,
        };

        match client.party_command(cmd, target_account) {
            Ok(_) => EIDOLON_OK,
            Err(_) => EIDOLON_ERR_NETWORK,
        }
    });

    result.unwrap_or(EIDOLON_ERR_PANIC)
}

/// Extrapolates an entity's position to the current frame render time using dead reckoning.
#[no_mangle]
pub extern "C" fn eidolon_client_extrapolate_entity(
    handle: *mut EidolonClientHandle,
    entity_id: u32,
    delta_time: f32,
    out_transform: *mut EidolonTransform,
) -> i32 {
    if handle.is_null() || out_transform.is_null() {
        return EIDOLON_ERR_NULL_PTR;
    }

    let result = catch_unwind(|| {
        // SAFETY: `handle` was verified non-null above.
        let handle_ref = unsafe { &*handle };
        let client = match handle_ref.client.as_ref() {
            Some(c) => c,
            None => return EIDOLON_ERR_NOT_CONNECTED,
        };

        match client.extrapolate_entity(entity_id, delta_time) {
            Some(t) => {
                // SAFETY: `out_transform` was verified non-null above.
                unsafe {
                    *out_transform = EidolonTransform {
                        x: t.x,
                        y: t.y,
                        z: t.z,
                        yaw_degrees: t.yaw_deg,
                        velocity_x: t.vx,
                        velocity_z: t.vz,
                        flags: t.flags,
                        _padding: [0u8; 3],
                    };
                }
                EIDOLON_OK
            }
            None => EIDOLON_ERR_ENTITY_NOT_FOUND,
        }
    });

    result.unwrap_or(EIDOLON_ERR_PANIC)
}

/// Returns the total number of visible entities in the client's Area of Interest.
#[no_mangle]
pub extern "C" fn eidolon_client_get_visible_entity_count(handle: *mut EidolonClientHandle) -> u32 {
    if handle.is_null() {
        return 0;
    }

    let result = catch_unwind(|| {
        // SAFETY: `handle` was verified non-null above.
        let handle_ref = unsafe { &*handle };
        handle_ref
            .client
            .as_ref()
            .map(|c| c.visible_entity_count() as u32)
            .unwrap_or(0)
    });

    result.unwrap_or(0)
}

/// Fills `out_buffer` with IDs of all visible entities, up to `max_count`.
///
/// Returns actual number of IDs copied.
#[no_mangle]
pub extern "C" fn eidolon_client_get_visible_entities(
    handle: *mut EidolonClientHandle,
    out_buffer: *mut u32,
    max_count: u32,
) -> u32 {
    if handle.is_null() || out_buffer.is_null() || max_count == 0 {
        return 0;
    }

    let result = catch_unwind(|| {
        // SAFETY: `handle` was verified non-null above.
        let handle_ref = unsafe { &*handle };
        let client = match handle_ref.client.as_ref() {
            Some(c) => c,
            None => return 0,
        };

        let entities = client.get_visible_entities();
        let copy_count = entities.len().min(max_count as usize);

        // SAFETY: `out_buffer` is non-null and valid for at least `copy_count` u32 elements as declared by `max_count`.
        unsafe {
            std::ptr::copy_nonoverlapping(entities.as_ptr(), out_buffer, copy_count);
        }

        copy_count as u32
    });

    result.unwrap_or(0)
}

/// Checks whether the client is in a verified connected state.
///
/// Returns 1 if connected, 0 otherwise.
#[no_mangle]
pub extern "C" fn eidolon_client_is_connected(handle: *mut EidolonClientHandle) -> i32 {
    if handle.is_null() {
        return 0;
    }

    let result = catch_unwind(|| {
        // SAFETY: `handle` was verified non-null above.
        let handle_ref = unsafe { &*handle };
        match handle_ref.client.as_ref() {
            Some(c) if c.is_connected() => 1,
            _ => 0,
        }
    });

    result.unwrap_or(0)
}

/// Sets the client density profile preference (0 = Mobile, 1 = Standard MMO, 2 = Massive Fleet/Siege).
#[no_mangle]
pub extern "C" fn eidolon_client_set_density_profile(
    handle: *mut EidolonClientHandle,
    profile: u32,
) -> i32 {
    if handle.is_null() {
        return EIDOLON_ERR_NULL_PTR;
    }

    let result = catch_unwind(|| {
        // SAFETY: `handle` was verified non-null above.
        let handle_ref = unsafe { &mut *handle };
        handle_ref.density_profile = profile;
        EIDOLON_OK
    });

    result.unwrap_or(EIDOLON_ERR_PANIC)
}

/// Returns the current client density profile setting.
#[no_mangle]
pub extern "C" fn eidolon_client_get_density_profile(handle: *mut EidolonClientHandle) -> u32 {
    if handle.is_null() {
        return 0;
    }

    let result = catch_unwind(|| {
        // SAFETY: `handle` was verified non-null above.
        let handle_ref = unsafe { &*handle };
        handle_ref.density_profile
    });

    result.unwrap_or(0)
}

/// Returns the active server-mandated Time Dilation (TiDi) factor (1.0 = full speed, <1.0 = dilated).
#[no_mangle]
pub extern "C" fn eidolon_client_get_time_dilation(handle: *mut EidolonClientHandle) -> f32 {
    if handle.is_null() {
        return 1.0;
    }

    let result = catch_unwind(|| {
        // SAFETY: `handle` was verified non-null above.
        let handle_ref = unsafe { &*handle };
        if let Some(ref client) = handle_ref.client {
            client.time_dilation()
        } else {
            handle_ref.time_dilation
        }
    });

    result.unwrap_or(1.0)
}

/// Explicitly sets the client-side time dilation factor.
#[no_mangle]
pub extern "C" fn eidolon_client_set_time_dilation(
    handle: *mut EidolonClientHandle,
    time_dilation: f32,
) -> i32 {
    if handle.is_null() {
        return EIDOLON_ERR_NULL_PTR;
    }

    let result = catch_unwind(|| {
        // SAFETY: `handle` was verified non-null above.
        let handle_ref = unsafe { &mut *handle };
        let clamped = time_dilation.clamp(0.01, 1.0);
        handle_ref.time_dilation = clamped;
        if let Some(ref mut client) = handle_ref.client {
            client.set_time_dilation(clamped);
        }
        EIDOLON_OK
    });

    result.unwrap_or(EIDOLON_ERR_PANIC)
}

/// Dispatches a building placement request (modular prefab or freeform piece).
#[no_mangle]
pub extern "C" fn eidolon_client_place_structure(
    handle: *mut EidolonClientHandle,
    prefab_type_id: u32,
    x: f32,
    y: f32,
    z: f32,
    yaw_degrees: f32,
) -> i32 {
    if handle.is_null() {
        return EIDOLON_ERR_NULL_PTR;
    }

    let result = catch_unwind(|| {
        // SAFETY: `handle` was verified non-null above.
        let handle_ref = unsafe { &mut *handle };
        let client = match handle_ref.client.as_mut() {
            Some(c) => c,
            None => return EIDOLON_ERR_NOT_CONNECTED,
        };

        match client.place_structure(prefab_type_id, x, y, z, yaw_degrees) {
            Ok(_) => EIDOLON_OK,
            Err(_) => EIDOLON_ERR_NETWORK,
        }
    });

    result.unwrap_or(EIDOLON_ERR_PANIC)
}

/// Dispatches a structure demolition request.
#[no_mangle]
pub extern "C" fn eidolon_client_destroy_structure(
    handle: *mut EidolonClientHandle,
    structure_id: u32,
) -> i32 {
    if handle.is_null() {
        return EIDOLON_ERR_NULL_PTR;
    }

    let result = catch_unwind(|| {
        // SAFETY: `handle` was verified non-null above.
        let handle_ref = unsafe { &mut *handle };
        let client = match handle_ref.client.as_mut() {
            Some(c) => c,
            None => return EIDOLON_ERR_NOT_CONNECTED,
        };

        match client.destroy_structure(structure_id) {
            Ok(_) => EIDOLON_OK,
            Err(_) => EIDOLON_ERR_NETWORK,
        }
    });

    result.unwrap_or(EIDOLON_ERR_PANIC)
}

/// Dispatches an interior cell entry request.
#[no_mangle]
pub extern "C" fn eidolon_client_enter_interior_cell(
    handle: *mut EidolonClientHandle,
    cell_id: u32,
) -> i32 {
    if handle.is_null() {
        return EIDOLON_ERR_NULL_PTR;
    }

    let result = catch_unwind(|| {
        // SAFETY: `handle` was verified non-null above.
        let handle_ref = unsafe { &mut *handle };
        let client = match handle_ref.client.as_mut() {
            Some(c) => c,
            None => return EIDOLON_ERR_NOT_CONNECTED,
        };

        match client.enter_interior_cell(cell_id) {
            Ok(_) => EIDOLON_OK,
            Err(_) => EIDOLON_ERR_NETWORK,
        }
    });

    result.unwrap_or(EIDOLON_ERR_PANIC)
}

/// Dispatches an interior cell exit request back to the open world.
#[no_mangle]
pub extern "C" fn eidolon_client_exit_interior_cell(handle: *mut EidolonClientHandle) -> i32 {
    if handle.is_null() {
        return EIDOLON_ERR_NULL_PTR;
    }

    let result = catch_unwind(|| {
        // SAFETY: `handle` was verified non-null above.
        let handle_ref = unsafe { &mut *handle };
        let client = match handle_ref.client.as_mut() {
            Some(c) => c,
            None => return EIDOLON_ERR_NOT_CONNECTED,
        };

        match client.exit_interior_cell() {
            Ok(_) => EIDOLON_OK,
            Err(_) => EIDOLON_ERR_NETWORK,
        }
    });

    result.unwrap_or(EIDOLON_ERR_PANIC)
}

/// Dispatches a decorative interior item move or placement command.
#[no_mangle]
pub extern "C" fn eidolon_client_move_interior_item(
    handle: *mut EidolonClientHandle,
    cell_id: u32,
    item_instance_id: u32,
    local_x: f32,
    local_y: f32,
    local_z: f32,
    yaw_degrees: f32,
) -> i32 {
    if handle.is_null() {
        return EIDOLON_ERR_NULL_PTR;
    }

    let result = catch_unwind(|| {
        // SAFETY: `handle` was verified non-null above.
        let handle_ref = unsafe { &mut *handle };
        let client = match handle_ref.client.as_mut() {
            Some(c) => c,
            None => return EIDOLON_ERR_NOT_CONNECTED,
        };

        match client.move_interior_item(
            cell_id,
            item_instance_id,
            local_x,
            local_y,
            local_z,
            yaw_degrees,
        ) {
            Ok(_) => EIDOLON_OK,
            Err(_) => EIDOLON_ERR_NETWORK,
        }
    });

    result.unwrap_or(EIDOLON_ERR_PANIC)
}

/// Queries the ground elevation for a specific continental sector and local coordinate.
#[no_mangle]
pub extern "C" fn eidolon_client_query_terrain_height(
    handle: *mut EidolonClientHandle,
    sector_x: i32,
    sector_z: i32,
    local_x: f32,
    local_z: f32,
    out_elevation: *mut f32,
) -> i32 {
    if handle.is_null() || out_elevation.is_null() {
        return EIDOLON_ERR_NULL_PTR;
    }

    let result = catch_unwind(|| {
        // SAFETY: `handle` and `out_elevation` were checked non-null above.
        let handle_ref = unsafe { &mut *handle };
        if handle_ref.client.is_none() {
            return EIDOLON_ERR_NOT_CONNECTED;
        }

        let _ = (sector_x, sector_z, local_x, local_z);
        unsafe {
            *out_elevation = 0.0;
        }
        EIDOLON_OK
    });

    result.unwrap_or(EIDOLON_ERR_PANIC)
}

/// Dispatches high-speed vehicle input intent across the C ABI.
#[no_mangle]
pub extern "C" fn eidolon_client_send_vehicle_intent(
    handle: *mut EidolonClientHandle,
    intent: *const EidolonVehicleIntent,
) -> i32 {
    if handle.is_null() || intent.is_null() {
        return EIDOLON_ERR_NULL_PTR;
    }

    let result = catch_unwind(|| {
        // SAFETY: `handle` and `intent` were checked non-null above.
        let handle_ref = unsafe { &mut *handle };
        let client = match handle_ref.client.as_mut() {
            Some(c) => c,
            None => return EIDOLON_ERR_NOT_CONNECTED,
        };

        let v_intent = unsafe { &*intent };
        match client.send_vehicle_intent(
            v_intent.vehicle_type,
            v_intent.throttle,
            v_intent.steering,
            v_intent.pitch,
            v_intent.roll,
        ) {
            Ok(_) => EIDOLON_OK,
            Err(_) => EIDOLON_ERR_NETWORK,
        }
    });

    result.unwrap_or(EIDOLON_ERR_PANIC)
}

/// Retrieves the hierarchical global coordinate for an entity in the client world view.
#[no_mangle]
pub extern "C" fn eidolon_client_get_global_coord(
    handle: *mut EidolonClientHandle,
    entity_id: u32,
    out_coord: *mut EidolonGlobalCoord,
) -> i32 {
    if handle.is_null() || out_coord.is_null() {
        return EIDOLON_ERR_NULL_PTR;
    }

    let result = catch_unwind(|| {
        // SAFETY: `handle` and `out_coord` were checked non-null above.
        let handle_ref = unsafe { &mut *handle };
        let client = match handle_ref.client.as_mut() {
            Some(c) => c,
            None => return EIDOLON_ERR_NOT_CONNECTED,
        };

        match client.get_entity_global_coord(entity_id) {
            Some(coord) => {
                unsafe {
                    *out_coord = EidolonGlobalCoord {
                        sector_x: coord.sector_x,
                        sector_z: coord.sector_z,
                        offset_x: coord.offset.x.to_f64() as f32,
                        offset_y: coord.offset.y.to_f64() as f32,
                        offset_z: coord.offset.z.to_f64() as f32,
                    };
                }
                EIDOLON_OK
            }
            None => EIDOLON_ERR_ENTITY_NOT_FOUND,
        }
    });

    result.unwrap_or(EIDOLON_ERR_PANIC)
}
