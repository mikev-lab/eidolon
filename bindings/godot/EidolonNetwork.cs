// EidolonNetwork.cs: Turnkey Autoload Network Manager for Godot 4 (.NET / C#)
// Manages client connection lifecycle, frame polling, and entity replication signals.

using System.Collections.Generic;
using Godot;

namespace Eidolon.GodotEngine
{
    [GlobalClass]
    public partial class EidolonNetwork : Node
    {
        public static EidolonNetwork Instance { get; private set; }

        [Signal] public delegate void ConnectedEventHandler(ulong accountId);
        [Signal] public delegate void DisconnectedEventHandler(int reasonCode);
        [Signal] public delegate void EntitySpawnedEventHandler(uint entityId, uint entityType, Vector3 position);
        [Signal] public delegate void EntityUpdatedEventHandler(uint entityId, Vector3 position, float yawDegrees);
        [Signal] public delegate void EntityDespawnedEventHandler(uint entityId);
        [Signal] public delegate void ChatMessageReceivedEventHandler(byte channel, uint senderId, string message);
        [Signal] public delegate void TimeDilationChangedEventHandler(float timeDilation);

        [Export] public EidolonGodotConfig Config { get; set; }
        [Export] public bool AutoConnectOnReady { get; set; } = true;
        [Export] public PackedScene DefaultEntityScene { get; set; }
        [Export] public Node3D EntityParentNode { get; set; }

        private EidolonClient _client;
        private readonly Dictionary<uint, EidolonEntityNode3D> _activeEntities = new Dictionary<uint, EidolonEntityNode3D>();

        public bool IsConnected => _client != null && _client.IsConnected;
        public float TimeDilation => _client != null ? _client.TimeDilation : 1.0f;

        public override void _Ready()
        {
            if (Instance != null && Instance != this)
            {
                QueueFree();
                return;
            }
            Instance = this;

            _client = new EidolonClient();
            BindClientEvents();

            if (AutoConnectOnReady)
            {
                Connect();
            }
        }

        public override void _ExitTree()
        {
            if (_client != null)
            {
                _client.Dispose();
                _client = null;
            }
            if (Instance == this)
            {
                Instance = null;
            }
        }

        public override void _Process(double delta)
        {
            if (_client != null)
            {
                _client.PollEvents();
            }
        }

        public bool Connect(string host = null, int? port = null, ulong? accountId = null)
        {
            if (_client == null)
            {
                return false;
            }

            string targetHost = host ?? (Config != null ? Config.ServerHost : "127.0.0.1");
            ushort targetPort = (ushort)(port ?? (Config != null ? Config.ServerPort : 7777));
            ulong targetAccount = accountId ?? (Config != null ? Config.DefaultAccountId : 1001);
            ulong ticketHigh = Config != null ? Config.TicketHigh : 0;
            ulong ticketLow = Config != null ? Config.TicketLow : 0;

            bool success = _client.Connect(targetHost, targetPort, targetAccount, ticketHigh, ticketLow);
            if (success && Config != null)
            {
                _client.DensityProfile = Config.DensityProfile;
            }
            return success;
        }

        public void Disconnect()
        {
            if (_client != null)
            {
                _client.Disconnect();
            }
            ClearEntities();
        }

        public bool SendIntent(float moveX, float moveZ, float yawDeg, byte flags)
        {
            if (_client != null && _client.IsConnected)
            {
                return _client.SendIntent(moveX, moveZ, yawDeg, flags);
            }
            return false;
        }

        public bool CastAbility(uint targetEntityId, uint abilityId)
        {
            if (_client != null && _client.IsConnected)
            {
                return _client.CastAbility(targetEntityId, abilityId);
            }
            return false;
        }

        public bool EquipItem(byte inventorySlot, byte equipSlot)
        {
            if (_client != null && _client.IsConnected)
            {
                return _client.EquipItem(inventorySlot, equipSlot);
            }
            return false;
        }

        public bool SendVehicleIntent(byte vehicleType, byte throttle, sbyte steering, sbyte pitch, sbyte roll)
        {
            if (_client != null && _client.IsConnected)
            {
                var intent = new EidolonVehicleIntent
                {
                    VehicleType = vehicleType,
                    Throttle = throttle,
                    Steering = steering,
                    Pitch = pitch,
                    Roll = roll
                };
                return _client.SendVehicleIntent(ref intent);
            }
            return false;
        }

        public bool QueryTerrainHeight(int sectorX, int sectorZ, float localX, float localZ, out float elevation)
        {
            elevation = 0.0f;
            if (_client != null)
            {
                return _client.QueryTerrainHeight(sectorX, sectorZ, localX, localZ, out elevation);
            }
            return false;
        }

        private void BindClientEvents()
        {
            _client.OnConnected += (account, port) =>
            {
                EmitSignal(SignalName.Connected, account);
            };

            _client.OnDisconnected += (reason) =>
            {
                ClearEntities();
                EmitSignal(SignalName.Disconnected, reason);
            };

            _client.OnEntitySpawned += (entityId, entityType, x, y, z, yaw) =>
            {
                Vector3 spawnPos = new Vector3(x, y, z);
                SpawnEntityNode(entityId, entityType, spawnPos, yaw);
                EmitSignal(SignalName.EntitySpawned, entityId, (uint)entityType, spawnPos);
            };

            _client.OnEntityUpdated += (entityId, x, y, z, yaw, flags) =>
            {
                if (_activeEntities.TryGetValue(entityId, out var node))
                {
                    node.UpdateTransform(x, y, z, yaw, 0.0f, 0.0f, flags);
                }
                EmitSignal(SignalName.EntityUpdated, entityId, new Vector3(x, y, z), yaw);
            };

            _client.OnEntityDespawned += (entityId) =>
            {
                DespawnEntityNode(entityId);
                EmitSignal(SignalName.EntityDespawned, entityId);
            };

            _client.OnChatMessage += (channel, senderId) =>
            {
                EmitSignal(SignalName.ChatMessageReceived, channel, senderId, "Chat message");
            };

            _client.OnTimeDilationChanged += (timeDilation) =>
            {
                EmitSignal(SignalName.TimeDilationChanged, timeDilation);
            };
        }

        private void SpawnEntityNode(uint entityId, byte entityType, Vector3 spawnPos, float yaw)
        {
            if (_activeEntities.ContainsKey(entityId))
            {
                return;
            }

            EidolonEntityNode3D node = null;
            if (DefaultEntityScene != null)
            {
                var instance = DefaultEntityScene.Instantiate();
                node = instance as EidolonEntityNode3D;
                if (node == null && instance is Node3D node3d)
                {
                    node = new EidolonEntityNode3D();
                    node3d.AddChild(node);
                }
            }

            if (node == null)
            {
                node = new EidolonEntityNode3D();
            }

            node.Name = $"EidolonEntity_{entityId}";

            Node parent = EntityParentNode ?? GetTree().CurrentScene ?? this;
            parent.AddChild(node);

            node.Initialize(entityId, entityType, spawnPos, yaw);
            _activeEntities.Add(entityId, node);
        }

        private void DespawnEntityNode(uint entityId)
        {
            if (_activeEntities.TryGetValue(entityId, out var node))
            {
                _activeEntities.Remove(entityId);
                if (node != null && IsInstanceValid(node))
                {
                    node.QueueFree();
                }
            }
        }

        private void ClearEntities()
        {
            foreach (var kvp in _activeEntities)
            {
                if (kvp.Value != null && IsInstanceValid(kvp.Value))
                {
                    kvp.Value.QueueFree();
                }
            }
            _activeEntities.Clear();
        }
    }
}
