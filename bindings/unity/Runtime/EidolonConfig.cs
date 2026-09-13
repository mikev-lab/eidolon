// EidolonConfig.cs: ScriptableObject Configuration Asset for Unity
// Allows game designers and engineers to configure environment profiles without modifying code.

using UnityEngine;

namespace Eidolon.Unity
{
    [CreateAssetMenu(fileName = "EidolonConfig", menuName = "Eidolon/Client Configuration")]
    public class EidolonConfig : ScriptableObject
    {
        [Header("Server Connection")]
        [Tooltip("Authoritative server hostname or IP address.")]
        public string ServerHost = "127.0.0.1";

        [Tooltip("Authoritative server UDP port.")]
        public ushort ServerPort = 7777;

        [Tooltip("Default account identifier for local test sessions.")]
        public ulong DefaultAccountId = 1001;

        [Tooltip("Session ticket cryptographic high 64 bits.")]
        public ulong TicketHigh = 0;

        [Tooltip("Session ticket cryptographic low 64 bits.")]
        public ulong TicketLow = 0;

        [Header("Bandwidth & Area of Interest")]
        [Tooltip("Bandwidth density profile (0 = Mobile, 1 = Standard MMO, 2 = Massive Fleet/Siege).")]
        [Range(0, 2)]
        public uint DensityProfile = EidolonDensityProfile.StandardMMO;

        [Header("Dead Reckoning & Smoothing")]
        [Tooltip("Interpolation delay buffer in seconds (e.g. 0.05s = 50ms / 1 server tick).")]
        [Range(0.01f, 0.20f)]
        public float InterpolationDelay = 0.05f;

        [Tooltip("Maximum duration in seconds to extrapolate with dead reckoning when packets are delayed.")]
        [Range(0.10f, 1.00f)]
        public float MaxExtrapolation = 0.50f;

        [Tooltip("Distance in meters that triggers an instant snap instead of smoothing (teleport/respawn).")]
        [Range(1.0f, 50.0f)]
        public float SnapDistanceThreshold = 10.0f;

        [Tooltip("Coordinate mapping convention between Eidolon world space and Unity scene space.")]
        public CoordinateConvention CoordinateConvention = CoordinateConvention.DirectParity;
    }
}
