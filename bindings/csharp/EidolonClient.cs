// EidolonClient.cs: Universal C# Bindings for Eidolon MMO Engine
// Compatible with Unity 2021+ (.NET Standard 2.1) and Godot 4 (.NET / Mono).
// Drop this file into your Unity Assets/Scripts or Godot C# project directory.

using System;
using System.Runtime.InteropServices;

namespace Eidolon
{
    /// <summary>
    /// Continuous 3D world transform representation.
    /// </summary>
    [StructLayout(LayoutKind.Sequential)]
    public struct EidolonTransform
    {
        public float X;
        public float Y;
        public float Z;
        public float YawDegrees;
        public float VelocityX;
        public float VelocityZ;
        public byte Flags;
        private byte _pad0;
        private byte _pad1;
        private byte _pad2;
    }

    /// <summary>
    /// Event data dispatched from the native Eidolon networking thread.
    /// </summary>
    [StructLayout(LayoutKind.Sequential)]
    public struct EidolonEvent
    {
        public uint EventType;
        public uint EntityId;
        public float X;
        public float Y;
        public float Z;
        public float YawDegrees;
        public ulong Param1;
        public ulong Param2;
    }

    /// <summary>
    /// High-level managed wrapper around native libeidolon client instance.
    /// </summary>
    public class EidolonClient : IDisposable
    {
        private const string LibName = "eidolon";

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        private delegate void NativeEventCallback(ref EidolonEvent evt, IntPtr userData);

        [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
        private static extern IntPtr eidolon_client_create();

        [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
        private static extern void eidolon_client_destroy(IntPtr handle);

        [DllImport(LibName, CallingConvention = CallingConvention.Cdecl, CharSet = CharSet.Ansi)]
        private static extern int eidolon_client_connect(
            IntPtr handle,
            string host,
            ushort port,
            ulong accountId,
            ulong ticketHigh,
            ulong ticketLow
        );

        [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
        private static extern void eidolon_client_disconnect(IntPtr handle);

        [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
        private static extern uint eidolon_client_poll_events(
            IntPtr handle,
            NativeEventCallback callback,
            IntPtr userData
        );

        [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
        private static extern int eidolon_client_send_intent(
            IntPtr handle,
            float moveX,
            float moveZ,
            float yawDeg,
            byte flags
        );

        [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
        private static extern int eidolon_client_send_action(
            IntPtr handle,
            byte actionType,
            uint targetId,
            uint param
        );

        [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
        private static extern int eidolon_client_cast_ability(
            IntPtr handle,
            uint targetId,
            uint abilityId
        );

        [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
        private static extern int eidolon_client_equip_item(
            IntPtr handle,
            byte inventorySlot,
            byte equipSlot
        );

        [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
        private static extern int eidolon_client_unequip_item(
            IntPtr handle,
            byte equipSlot
        );

        [DllImport(LibName, CallingConvention = CallingConvention.Cdecl, CharSet = CharSet.Ansi)]
        private static extern int eidolon_client_send_chat(
            IntPtr handle,
            byte channel,
            uint targetId,
            string text
        );

        [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
        private static extern int eidolon_client_party_command(
            IntPtr handle,
            byte cmd,
            ulong targetAccount
        );

        [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
        private static extern int eidolon_client_extrapolate_entity(
            IntPtr handle,
            uint entityId,
            float deltaTime,
            out EidolonTransform outTransform
        );

        [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
        private static extern uint eidolon_client_get_visible_entity_count(IntPtr handle);

        [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
        private static extern uint eidolon_client_get_visible_entities(
            IntPtr handle,
            [Out] uint[] outBuffer,
            uint maxCount
        );

        [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
        private static extern int eidolon_client_is_connected(IntPtr handle);

        private IntPtr _handle;
        private readonly NativeEventCallback _callbackDelegate;
        private bool _disposed;

        // Public Managed Events
        public event Action<ulong, ushort> OnConnected;
        public event Action<int> OnDisconnected;
        public event Action<uint, byte, float, float, float, float> OnEntitySpawned;
        public event Action<uint> OnEntityDespawned;
        public event Action<uint, float, float, float, float, byte> OnEntityUpdated;
        public event Action<uint, uint, byte, uint> OnCombatAction;
        public event Action<uint, uint, uint> OnLootAcquired;
        public event Action<uint, uint, uint> OnCastStarted;
        public event Action<uint, uint, byte> OnCastInterrupted;
        public event Action<uint, uint> OnCastCompleted;
        public event Action<byte, uint> OnChatMessage;
        public event Action<uint, byte, uint> OnEquipmentChanged;
        public event Action<ulong, ulong, byte> OnPartyUpdated;

        public bool IsConnected => _handle != IntPtr.Zero && eidolon_client_is_connected(_handle) == 1;

        public EidolonClient()
        {
            _handle = eidolon_client_create();
            if (_handle == IntPtr.Zero)
            {
                throw new InvalidOperationException("Failed to allocate native EidolonClient handle.");
            }
            _callbackDelegate = HandleNativeEvent;
        }

        public bool Connect(string host, ushort port, ulong accountId, ulong ticketHigh, ulong ticketLow)
        {
            ThrowIfDisposed();
            int res = eidolon_client_connect(_handle, host, port, accountId, ticketHigh, ticketLow);
            return res == 0;
        }

        public void Disconnect()
        {
            if (_handle != IntPtr.Zero)
            {
                eidolon_client_disconnect(_handle);
            }
        }

        public uint PollEvents()
        {
            ThrowIfDisposed();
            return eidolon_client_poll_events(_handle, _callbackDelegate, IntPtr.Zero);
        }

        public bool SendMovementIntent(float moveX, float moveZ, float yawDeg, byte flags)
        {
            ThrowIfDisposed();
            return eidolon_client_send_intent(_handle, moveX, moveZ, yawDeg, flags) == 0;
        }

        public bool SendAction(byte actionType, uint targetId, uint param)
        {
            ThrowIfDisposed();
            return eidolon_client_send_action(_handle, actionType, targetId, param) == 0;
        }

        public bool CastAbility(uint targetId, uint abilityId)
        {
            ThrowIfDisposed();
            return eidolon_client_cast_ability(_handle, targetId, abilityId) == 0;
        }

        public bool EquipItem(byte inventorySlot, byte equipSlot)
        {
            ThrowIfDisposed();
            return eidolon_client_equip_item(_handle, inventorySlot, equipSlot) == 0;
        }

        public bool UnequipItem(byte equipSlot)
        {
            ThrowIfDisposed();
            return eidolon_client_unequip_item(_handle, equipSlot) == 0;
        }

        public bool SendChat(byte channel, uint targetId, string text)
        {
            ThrowIfDisposed();
            return eidolon_client_send_chat(_handle, channel, targetId, text) == 0;
        }

        public bool PartyCommand(byte cmd, ulong targetAccount)
        {
            ThrowIfDisposed();
            return eidolon_client_party_command(_handle, cmd, targetAccount) == 0;
        }

        public bool TryExtrapolateEntity(uint entityId, float deltaTime, out EidolonTransform transform)
        {
            ThrowIfDisposed();
            int res = eidolon_client_extrapolate_entity(_handle, entityId, deltaTime, out transform);
            return res == 0;
        }

        public uint[] GetVisibleEntityIds()
        {
            ThrowIfDisposed();
            uint count = eidolon_client_get_visible_entity_count(_handle);
            if (count == 0) return Array.Empty<uint>();

            uint[] buffer = new uint[count];
            uint written = eidolon_client_get_visible_entities(_handle, buffer, count);
            if (written < count)
            {
                Array.Resize(ref buffer, (int)written);
            }
            return buffer;
        }

        private void HandleNativeEvent(ref EidolonEvent evt, IntPtr userData)
        {
            switch (evt.EventType)
            {
                case 1: // Connected
                    OnConnected?.Invoke(evt.Param1, (ushort)evt.Param2);
                    break;
                case 2: // Disconnected
                    OnDisconnected?.Invoke((int)evt.Param1);
                    break;
                case 3: // EntitySpawned
                    OnEntitySpawned?.Invoke(evt.EntityId, (byte)evt.Param1, evt.X, evt.Y, evt.Z, evt.YawDegrees);
                    break;
                case 4: // EntityDespawned
                    OnEntityDespawned?.Invoke(evt.EntityId);
                    break;
                case 5: // EntityUpdated
                    OnEntityUpdated?.Invoke(evt.EntityId, evt.X, evt.Y, evt.Z, evt.YawDegrees, (byte)evt.Param1);
                    break;
                case 6: // CombatAction
                    uint targetId = (uint)evt.Param1;
                    byte actionType = (byte)(evt.Param2 >> 32);
                    uint value = (uint)(evt.Param2 & 0xFFFFFFFF);
                    OnCombatAction?.Invoke(evt.EntityId, targetId, actionType, value);
                    break;
                case 7: // LootAcquired
                    uint itemId = (uint)evt.Param1;
                    uint amount = (uint)evt.Param2;
                    OnLootAcquired?.Invoke(evt.EntityId, itemId, amount);
                    break;
                case 8: // CastStarted
                    OnCastStarted?.Invoke(evt.EntityId, (uint)evt.Param1, (uint)evt.Param2);
                    break;
                case 9: // CastInterrupted
                    OnCastInterrupted?.Invoke(evt.EntityId, (uint)evt.Param1, (byte)evt.Param2);
                    break;
                case 10: // CastCompleted
                    OnCastCompleted?.Invoke(evt.EntityId, (uint)evt.Param1);
                    break;
                case 11: // ChatMessage
                    OnChatMessage?.Invoke((byte)evt.Param1, (uint)evt.Param2);
                    break;
                case 12: // EquipmentChanged
                    OnEquipmentChanged?.Invoke(evt.EntityId, (byte)evt.Param1, (uint)evt.Param2);
                    break;
                case 13: // PartyUpdated
                    ulong leaderId = evt.Param2 >> 8;
                    byte memberCount = (byte)(evt.Param2 & 0xFF);
                    OnPartyUpdated?.Invoke(evt.Param1, leaderId, memberCount);
                    break;
            }
        }

        private void ThrowIfDisposed()
        {
            if (_disposed || _handle == IntPtr.Zero)
            {
                throw new ObjectDisposedException(nameof(EidolonClient));
            }
        }

        public void Dispose()
        {
            if (!_disposed)
            {
                if (_handle != IntPtr.Zero)
                {
                    eidolon_client_destroy(_handle);
                    _handle = IntPtr.Zero;
                }
                _disposed = true;
                GC.SuppressFinalize(this);
            }
        }

        ~EidolonClient()
        {
            Dispose();
        }
    }
}
