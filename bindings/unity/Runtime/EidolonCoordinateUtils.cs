// EidolonCoordinateUtils.cs: Coordinate Conversion and Smoothing Utilities for Unity
// Handles spatial mapping between Eidolon right-handed coordinates and Unity left-handed space.

using UnityEngine;

namespace Eidolon.Unity
{
    /// <summary>
    /// Configurable coordinate mapping conventions.
    /// </summary>
    public enum CoordinateConvention
    {
        /// <summary>
        /// Direct 1:1 mapping: X = x, Y = y, Z = z.
        /// </summary>
        DirectParity = 0,

        /// <summary>
        /// Inverted Z axis for right-handed to left-handed conversion: X = x, Y = y, Z = -z.
        /// </summary>
        InvertZ = 1,

        /// <summary>
        /// Swapped axes (Z forward, X right): X = z, Y = y, Z = x.
        /// </summary>
        SwappedXZ = 2,
    }

    /// <summary>
    /// Static utility methods for transforming positions, rotations, and vectors.
    /// </summary>
    public static class EidolonCoordinateUtils
    {
        /// <summary>
        /// Converts Eidolon coordinates (meters) to Unity Vector3.
        /// </summary>
        public static Vector3 ToUnityPosition(float x, float y, float z, CoordinateConvention convention = CoordinateConvention.DirectParity)
        {
            switch (convention)
            {
                case CoordinateConvention.InvertZ:
                    return new Vector3(x, y, -z);
                case CoordinateConvention.SwappedXZ:
                    return new Vector3(z, y, x);
                case CoordinateConvention.DirectParity:
                default:
                    return new Vector3(x, y, z);
            }
        }

        /// <summary>
        /// Converts Unity Vector3 position back into Eidolon coordinate components.
        /// </summary>
        public static void ToEidolonPosition(Vector3 unityPosition, out float x, out float y, out float z, CoordinateConvention convention = CoordinateConvention.DirectParity)
        {
            switch (convention)
            {
                case CoordinateConvention.InvertZ:
                    x = unityPosition.x;
                    y = unityPosition.y;
                    z = -unityPosition.z;
                    break;
                case CoordinateConvention.SwappedXZ:
                    x = unityPosition.z;
                    y = unityPosition.y;
                    z = unityPosition.x;
                    break;
                case CoordinateConvention.DirectParity:
                default:
                    x = unityPosition.x;
                    y = unityPosition.y;
                    z = unityPosition.z;
                    break;
            }
        }

        /// <summary>
        /// Converts quantized yaw angle (degrees) to Unity Quaternion rotation.
        /// </summary>
        public static Quaternion ToUnityRotation(float yawDegrees, CoordinateConvention convention = CoordinateConvention.DirectParity)
        {
            float adjustedYaw = yawDegrees;
            if (convention == CoordinateConvention.InvertZ)
            {
                adjustedYaw = -yawDegrees;
            }
            return Quaternion.Euler(0.0f, adjustedYaw, 0.0f);
        }

        /// <summary>
        /// Derives quantized yaw angle (degrees) from Unity Quaternion.
        /// </summary>
        public static float ToEidolonYaw(Quaternion unityRotation, CoordinateConvention convention = CoordinateConvention.DirectParity)
        {
            float yaw = unityRotation.eulerAngles.y;
            if (convention == CoordinateConvention.InvertZ)
            {
                yaw = -yaw;
            }
            return Mathf.Repeat(yaw, 360.0f);
        }

        /// <summary>
        /// Shortest-path angle interpolation between two degree headings.
        /// </summary>
        public static float SmoothAngle(float currentDeg, float targetDeg, float speed, float deltaTime)
        {
            return Mathf.LerpAngle(currentDeg, targetDeg, 1.0f - Mathf.Exp(-speed * deltaTime));
        }
    }
}
