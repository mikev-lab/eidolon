// EidolonEntityView.cs: Smooth Remote Entity Replication View for Unity
// Decouples 60/120/144 FPS client rendering from 20 Hz server updates via dead reckoning.

using UnityEngine;

namespace Eidolon.Unity
{
    /// <summary>
    /// Attached to remote replicated entity prefabs to provide smooth visual interpolation.
    /// </summary>
    public class EidolonEntityView : MonoBehaviour
    {
        [Header("Entity State")]
        [SerializeField] private uint _entityId;
        [SerializeField] private uint _entityType;
        [SerializeField] private Vector3 _targetPosition;
        [SerializeField] private Quaternion _targetRotation;
        [SerializeField] private Vector3 _velocity;
        [SerializeField] private bool _isGrounded;

        [Header("Smoothing Parameters")]
        [SerializeField] private float _positionSmoothingSpeed = 15.0f;
        [SerializeField] private float _rotationSmoothingSpeed = 12.0f;
        [SerializeField] private float _snapDistanceThreshold = 10.0f;
        [SerializeField] private CoordinateConvention _convention = CoordinateConvention.DirectParity;

        private float _timeSinceLastUpdate;
        private bool _initialized;

        public uint EntityId => _entityId;
        public uint EntityType => _entityType;
        public Vector3 Velocity => _velocity;
        public bool IsGrounded => _isGrounded;

        /// <summary>
        /// Initializes the entity view with authoritative spawn parameters.
        /// </summary>
        public void Initialize(
            uint entityId,
            uint entityType,
            Vector3 initialPosition,
            Quaternion initialRotation,
            EidolonConfig config = null)
        {
            _entityId = entityId;
            _entityType = entityType;
            _targetPosition = initialPosition;
            _targetRotation = initialRotation;
            transform.position = initialPosition;
            transform.rotation = initialRotation;
            _timeSinceLastUpdate = 0.0f;
            _initialized = true;

            if (config != null)
            {
                _snapDistanceThreshold = config.SnapDistanceThreshold;
                _convention = config.CoordinateConvention;
            }
        }

        /// <summary>
        /// Ingests a new authoritative transform update received from the network.
        /// </summary>
        public void UpdateTransform(
            float x,
            float y,
            float z,
            float yawDegrees,
            float vx,
            float vz,
            byte flags)
        {
            Vector3 newPosition = EidolonCoordinateUtils.ToUnityPosition(x, y, z, _convention);
            Quaternion newRotation = EidolonCoordinateUtils.ToUnityRotation(yawDegrees, _convention);

            // Instant snap if displacement exceeds threshold (e.g. teleport, respawn)
            float distSq = (transform.position - newPosition).sqrMagnitude;
            if (distSq > _snapDistanceThreshold * _snapDistanceThreshold || !_initialized)
            {
                transform.position = newPosition;
                transform.rotation = newRotation;
                _targetPosition = newPosition;
                _targetRotation = newRotation;
            }
            else
            {
                _targetPosition = newPosition;
                _targetRotation = newRotation;
            }

            _velocity = new Vector3(vx, 0.0f, vz);
            _isGrounded = (flags & 0x01) != 0;
            _timeSinceLastUpdate = 0.0f;
            _initialized = true;
        }

        private void Update()
        {
            if (!_initialized)
            {
                return;
            }

            _timeSinceLastUpdate += Time.deltaTime;

            // Dead reckoning forward projection if update is overdue
            Vector3 projectedTarget = _targetPosition;
            if (_timeSinceLastUpdate > 0.05f && _velocity.sqrMagnitude > 0.01f)
            {
                float extraTime = Mathf.Min(_timeSinceLastUpdate - 0.05f, 0.50f);
                projectedTarget += _velocity * extraTime;
            }

            // Exponential smoothing to eliminate jitter at high refresh rates
            float posBlend = 1.0f - Mathf.Exp(-_positionSmoothingSpeed * Time.deltaTime);
            transform.position = Vector3.Lerp(transform.position, projectedTarget, posBlend);

            float rotBlend = 1.0f - Mathf.Exp(-_rotationSmoothingSpeed * Time.deltaTime);
            transform.rotation = Quaternion.Slerp(transform.rotation, _targetRotation, rotBlend);
        }
    }
}
