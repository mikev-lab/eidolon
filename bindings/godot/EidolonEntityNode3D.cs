// EidolonEntityNode3D.cs: Smooth Remote Entity Replication Node for Godot 4
// Exploits direct 1:1 right-handed Y-up coordinate parity between Eidolon and Godot.

using Godot;

namespace Eidolon.GodotEngine
{
    [GlobalClass]
    public partial class EidolonEntityNode3D : Node3D
    {
        [Export] public uint EntityId { get; set; }
        [Export] public uint EntityType { get; set; }
        [Export] public float PositionSmoothingSpeed { get; set; } = 15.0f;
        [Export] public float RotationSmoothingSpeed { get; set; } = 12.0f;
        [Export] public float SnapDistanceThreshold { get; set; } = 10.0f;

        private Vector3 _targetPosition;
        private float _targetYawDegrees;
        private Vector3 _velocity;
        private double _timeSinceLastUpdate;
        private bool _initialized;

        public Vector3 Velocity => _velocity;

        public void Initialize(uint entityId, uint entityType, Vector3 initialPosition, float initialYawDegrees)
        {
            EntityId = entityId;
            EntityType = entityType;
            _targetPosition = initialPosition;
            _targetYawDegrees = initialYawDegrees;
            GlobalPosition = initialPosition;
            RotationDegrees = new Vector3(0.0f, initialYawDegrees, 0.0f);
            _timeSinceLastUpdate = 0.0;
            _initialized = true;
        }

        public void UpdateTransform(float x, float y, float z, float yawDegrees, float vx, float vz, byte flags)
        {
            Vector3 newPosition = new Vector3(x, y, z);

            float distSq = GlobalPosition.DistanceSquaredTo(newPosition);
            if (distSq > SnapDistanceThreshold * SnapDistanceThreshold || !_initialized)
            {
                GlobalPosition = newPosition;
                RotationDegrees = new Vector3(0.0f, yawDegrees, 0.0f);
                _targetPosition = newPosition;
                _targetYawDegrees = yawDegrees;
            }
            else
            {
                _targetPosition = newPosition;
                _targetYawDegrees = yawDegrees;
            }

            _velocity = new Vector3(vx, 0.0f, vz);
            _timeSinceLastUpdate = 0.0;
            _initialized = true;
        }

        public override void _Process(double delta)
        {
            if (!_initialized)
            {
                return;
            }

            _timeSinceLastUpdate += delta;

            Vector3 projectedTarget = _targetPosition;
            if (_timeSinceLastUpdate > 0.05 && _velocity.LengthSquared() > 0.01f)
            {
                float extraTime = Mathf.Min((float)(_timeSinceLastUpdate - 0.05), 0.50f);
                projectedTarget += _velocity * extraTime;
            }

            float posBlend = 1.0f - Mathf.Exp(-PositionSmoothingSpeed * (float)delta);
            GlobalPosition = GlobalPosition.Lerp(projectedTarget, posBlend);

            float currentYaw = RotationDegrees.Y;
            float rotBlend = 1.0f - Mathf.Exp(-RotationSmoothingSpeed * (float)delta);
            float smoothYaw = Mathf.LerpAngle(Mathf.DegToRad(currentYaw), Mathf.DegToRad(_targetYawDegrees), rotBlend);
            Rotation = new Vector3(0.0f, smoothYaw, 0.0f);
        }
    }
}
