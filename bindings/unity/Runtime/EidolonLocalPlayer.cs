// EidolonLocalPlayer.cs: Local Character Input Sampling & 20 Hz Intent Dispatcher for Unity
// Strictly enforces 20 Hz intent cadence to respect the sub-1.2 KB/s wire budget.

using UnityEngine;

namespace Eidolon.Unity
{
    /// <summary>
    /// Attached to the local player GameObject to sample input and transmit movement intents.
    /// </summary>
    public class EidolonLocalPlayer : MonoBehaviour
    {
        [Header("Movement & Camera")]
        [Tooltip("Optional camera reference to calculate camera-relative movement vectors.")]
        [SerializeField] private Camera _playerCamera;

        [Tooltip("Movement speed in meters per second.")]
        [SerializeField] private float _moveSpeed = 6.0f;

        [Tooltip("Transmit rate in Hz (default 20 Hz / 50ms interval).")]
        [Range(10, 30)]
        [SerializeField] private int _intentRateHz = 20;

        [Header("Action Flags")]
        [SerializeField] private KeyCode _jumpKey = KeyCode.Space;
        [SerializeField] private KeyCode _sprintKey = KeyCode.LeftShift;

        private float _lastIntentTime;
        private float _intentInterval;

        private void Awake()
        {
            if (_playerCamera == null)
            {
                _playerCamera = Camera.main;
            }
            _intentInterval = 1.0f / _intentRateHz;
        }

        private void Update()
        {
            if (Time.time - _lastIntentTime < _intentInterval)
            {
                return;
            }

            _lastIntentTime = Time.time;
            SampleAndSendIntent();
        }

        private void SampleAndSendIntent()
        {
            if (EidolonNetworkManager.Instance == null || !EidolonNetworkManager.Instance.IsConnected)
            {
                return;
            }

            float inputH = Input.GetAxisRaw("Horizontal");
            float inputV = Input.GetAxisRaw("Vertical");

            Vector3 moveDir = Vector3.zero;
            if (_playerCamera != null)
            {
                Vector3 camForward = Vector3.ProjectOnPlane(_playerCamera.transform.forward, Vector3.up).normalized;
                Vector3 camRight = Vector3.ProjectOnPlane(_playerCamera.transform.right, Vector3.up).normalized;
                moveDir = (camForward * inputV + camRight * inputH).normalized;
            }
            else
            {
                moveDir = new Vector3(inputH, 0.0f, inputV).normalized;
            }

            byte flags = 0;
            if (Input.GetKey(_jumpKey))
            {
                flags |= 0x01; // Jump flag
            }
            if (Input.GetKey(_sprintKey))
            {
                flags |= 0x02; // Sprint flag
            }

            float currentYaw = transform.eulerAngles.y;
            if (moveDir.sqrMagnitude > 0.001f)
            {
                currentYaw = Mathf.Atan2(moveDir.x, moveDir.z) * Mathf.Rad2Deg;
                transform.rotation = Quaternion.Euler(0.0f, currentYaw, 0.0f);
            }

            Vector3 velocity = moveDir * _moveSpeed;

            EidolonNetworkManager.Instance.SendIntent(velocity.x, velocity.z, currentYaw, flags);
        }

        /// <summary>
        /// Dispatches an authoritative spell or ability cast command.
        /// </summary>
        public bool CastAbility(uint targetEntityId, uint abilityId)
        {
            if (EidolonNetworkManager.Instance != null)
            {
                return EidolonNetworkManager.Instance.CastAbility(targetEntityId, abilityId);
            }
            return false;
        }

        /// <summary>
        /// Dispatches an equipment change command.
        /// </summary>
        public bool EquipItem(byte inventorySlot, byte equipSlot)
        {
            if (EidolonNetworkManager.Instance != null)
            {
                return EidolonNetworkManager.Instance.EquipItem(inventorySlot, equipSlot);
            }
            return false;
        }

        /// <summary>
        /// Dispatches high-speed vehicle input intent.
        /// </summary>
        public bool SendVehicleIntent(byte vehicleType, byte throttle, sbyte steering, sbyte pitch, sbyte roll)
        {
            if (EidolonNetworkManager.Instance != null)
            {
                return EidolonNetworkManager.Instance.SendVehicleIntent(vehicleType, throttle, steering, pitch, roll);
            }
            return false;
        }
    }
}
