// EidolonNetworkManager.cs: Turnkey Inspector-Configurable Network Manager for Unity
// Manages connection lifecycle, event polling, and entity prefab instantiation.

using System;
using System.Collections.Generic;
using UnityEngine;
using UnityEngine.Events;

namespace Eidolon.Unity
{
    [Serializable]
    public struct EntityPrefabMapping
    {
        public uint EntityType;
        public GameObject Prefab;
    }

    /// <summary>
    /// Central network manager singleton for Unity MMO clients.
    /// </summary>
    public class EidolonNetworkManager : MonoBehaviour
    {
        public static EidolonNetworkManager Instance { get; private set; }

        [Header("Configuration")]
        [SerializeField] private EidolonConfig _config;
        [SerializeField] private bool _autoConnectOnStart = true;
        [SerializeField] private ulong _accountId = 1001;

        [Header("Entity Spawning Factory")]
        [Tooltip("Maps entity type IDs to GameObject prefabs for automatic instantiation.")]
        [SerializeField] private List<EntityPrefabMapping> _prefabMappings = new List<EntityPrefabMapping>();

        [Tooltip("Fallback prefab if no specific entity type mapping exists.")]
        [SerializeField] private GameObject _defaultEntityPrefab;

        [Tooltip("Optional parent transform to organize spawned entity GameObjects in Hierarchy.")]
        [SerializeField] private Transform _entityContainer;

        [Header("Debug Visualization")]
        [SerializeField] private bool _showDebugGizmos = true;
        [SerializeField] private Color _gizmoColor = new Color(0.0f, 0.8f, 1.0f, 0.4f);

        [Header("Unity Events")]
        public UnityEvent<ulong> OnConnected = new UnityEvent<ulong>();
        public UnityEvent<int> OnDisconnected = new UnityEvent<int>();
        public UnityEvent<uint, byte> OnEntitySpawned = new UnityEvent<uint, byte>();
        public UnityEvent<uint> OnEntityDespawned = new UnityEvent<uint>();
        public UnityEvent<byte, string> OnChatMessageReceived = new UnityEvent<byte, string>();
        public UnityEvent<float> OnTimeDilationChanged = new UnityEvent<float>();

        private EidolonClient _client;
        private readonly Dictionary<uint, EidolonEntityView> _activeEntities = new Dictionary<uint, EidolonEntityView>();
        private readonly Dictionary<uint, GameObject> _prefabLookup = new Dictionary<uint, GameObject>();

        public bool IsConnected => _client != null && _client.IsConnected;
        public float TimeDilation => _client != null ? _client.TimeDilation : 1.0f;

        private void Awake()
        {
            if (Instance != null && Instance != this)
            {
                Destroy(gameObject);
                return;
            }
            Instance = this;
            DontDestroyOnLoad(gameObject);

            // Populate prefab fast-lookup table
            foreach (var mapping in _prefabMappings)
            {
                if (mapping.Prefab != null && !_prefabLookup.ContainsKey(mapping.EntityType))
                {
                    _prefabLookup.Add(mapping.EntityType, mapping.Prefab);
                }
            }

            _client = new EidolonClient();
            BindClientEvents();
        }

        private void Start()
        {
            if (_autoConnectOnStart)
            {
                Connect();
            }
        }

        private void OnDestroy()
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

        private void Update()
        {
            if (_client != null)
            {
                _client.PollEvents();
            }
        }

        /// <summary>
        /// Connects to the server using the active configuration.
        /// </summary>
        public bool Connect(string host = null, ushort? port = null, ulong? accountId = null)
        {
            if (_client == null)
            {
                return false;
            }

            string targetHost = host ?? (_config != null ? _config.ServerHost : "127.0.0.1");
            ushort targetPort = port ?? (_config != null ? _config.ServerPort : (ushort)7777);
            ulong targetAccount = accountId ?? _accountId;
            ulong ticketHigh = _config != null ? _config.TicketHigh : 0;
            ulong ticketLow = _config != null ? _config.TicketLow : 0;

            bool success = _client.Connect(targetHost, targetPort, targetAccount, ticketHigh, ticketLow);
            if (success && _config != null)
            {
                _client.DensityProfile = _config.DensityProfile;
            }
            return success;
        }

        /// <summary>
        /// Disconnects from the authoritative server and cleans up active entities.
        /// </summary>
        public void Disconnect()
        {
            if (_client != null)
            {
                _client.Disconnect();
            }
            ClearEntities();
        }

        /// <summary>
        /// Transmits continuous movement intent inputs to the server.
        /// </summary>
        public bool SendIntent(float moveX, float moveZ, float yawDeg, byte flags)
        {
            if (_client != null && _client.IsConnected)
            {
                return _client.SendIntent(moveX, moveZ, yawDeg, flags);
            }
            return false;
        }

        /// <summary>
        /// Dispatches an authoritative spell or ability cast command.
        /// </summary>
        public bool CastAbility(uint targetEntityId, uint abilityId)
        {
            if (_client != null && _client.IsConnected)
            {
                return _client.CastAbility(targetEntityId, abilityId);
            }
            return false;
        }

        /// <summary>
        /// Dispatches an equipment change command.
        /// </summary>
        public bool EquipItem(byte inventorySlot, byte equipSlot)
        {
            if (_client != null && _client.IsConnected)
            {
                return _client.EquipItem(inventorySlot, equipSlot);
            }
            return false;
        }

        /// <summary>
        /// Dispatches high-speed vehicle input intent.
        /// </summary>
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

        /// <summary>
        /// Queries the macro terrain height at a specific sector and local offset.
        /// </summary>
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
                OnConnected.Invoke(account);
            };

            _client.OnDisconnected += (reason) =>
            {
                ClearEntities();
                OnDisconnected.Invoke(reason);
            };

            _client.OnEntitySpawned += (entityId, entityType, x, y, z, yaw) =>
            {
                SpawnEntityView(entityId, entityType, x, y, z, yaw);
                OnEntitySpawned.Invoke(entityId, entityType);
            };

            _client.OnEntityUpdated += (entityId, x, y, z, yaw, flags) =>
            {
                if (_activeEntities.TryGetValue(entityId, out var view))
                {
                    view.UpdateTransform(x, y, z, yaw, 0.0f, 0.0f, flags);
                }
            };

            _client.OnEntityDespawned += (entityId) =>
            {
                DespawnEntityView(entityId);
                OnEntityDespawned.Invoke(entityId);
            };

            _client.OnChatMessage += (channel, senderId) =>
            {
                OnChatMessageReceived.Invoke(channel, $"Entity {senderId}");
            };

            _client.OnTimeDilationChanged += (timeDilation) =>
            {
                OnTimeDilationChanged.Invoke(timeDilation);
            };
        }

        private void SpawnEntityView(uint entityId, uint entityType, float x, float y, float z, float yaw)
        {
            if (_activeEntities.ContainsKey(entityId))
            {
                return;
            }

            CoordinateConvention convention = _config != null ? _config.CoordinateConvention : CoordinateConvention.DirectParity;
            Vector3 spawnPos = EidolonCoordinateUtils.ToUnityPosition(x, y, z, convention);
            Quaternion spawnRot = EidolonCoordinateUtils.ToUnityRotation(yaw, convention);

            GameObject prefabToSpawn = null;
            if (!_prefabLookup.TryGetValue(entityType, out prefabToSpawn))
            {
                prefabToSpawn = _defaultEntityPrefab;
            }

            GameObject entityObj;
            if (prefabToSpawn != null)
            {
                entityObj = Instantiate(prefabToSpawn, spawnPos, spawnRot, _entityContainer);
            }
            else
            {
                entityObj = GameObject.CreatePrimitive(PrimitiveType.Capsule);
                entityObj.transform.position = spawnPos;
                entityObj.transform.rotation = spawnRot;
                if (_entityContainer != null)
                {
                    entityObj.transform.SetParent(_entityContainer);
                }
            }

            entityObj.name = $"EidolonEntity_{entityId}_Type{entityType}";

            EidolonEntityView view = entityObj.GetComponent<EidolonEntityView>();
            if (view == null)
            {
                view = entityObj.AddComponent<EidolonEntityView>();
            }

            view.Initialize(entityId, entityType, spawnPos, spawnRot, _config);
            _activeEntities.Add(entityId, view);
        }

        private void DespawnEntityView(uint entityId)
        {
            if (_activeEntities.TryGetValue(entityId, out var view))
            {
                _activeEntities.Remove(entityId);
                if (view != null && view.gameObject != null)
                {
                    Destroy(view.gameObject);
                }
            }
        }

        private void ClearEntities()
        {
            foreach (var kvp in _activeEntities)
            {
                if (kvp.Value != null && kvp.Value.gameObject != null)
                {
                    Destroy(kvp.Value.gameObject);
                }
            }
            _activeEntities.Clear();
        }

        private void OnDrawGizmos()
        {
            if (!_showDebugGizmos)
            {
                return;
            }

            Gizmos.color = _gizmoColor;
            foreach (var kvp in _activeEntities)
            {
                if (kvp.Value != null)
                {
                    Gizmos.DrawWireSphere(kvp.Value.transform.position, 1.0f);
                    if (kvp.Value.Velocity.sqrMagnitude > 0.01f)
                    {
                        Gizmos.DrawRay(kvp.Value.transform.position, kvp.Value.Velocity);
                    }
                }
            }
        }
    }
}
