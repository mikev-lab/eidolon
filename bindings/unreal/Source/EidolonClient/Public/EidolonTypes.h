// EidolonTypes.h: Unreal Engine 5 Blueprint Types & Coordinate Utilities

#pragma once

#include "CoreMinimal.h"
#include "EidolonTypes.generated.h"

/**
 * Bandwidth and spatial frequency tier density profiles.
 */
UENUM(BlueprintType)
enum class EEidolonDensityProfile : uint8
{
    Mobile = 0             UMETA(DisplayName = "Mobile (Budget Egress)"),
    StandardMMO = 1        UMETA(DisplayName = "Standard MMO (Balanced)"),
    MassiveFleet = 2       UMETA(DisplayName = "Massive Fleet / Siege")
};

/**
 * Coordinate mapping conventions between Eidolon world space and Unreal Engine.
 */
UENUM(BlueprintType)
enum class EEidolonCoordinateConvention : uint8
{
    // Unreal standard: Centimeters, Z-up, X-forward (X_ue = Z_eidolon * 100, Y_ue = X_eidolon * 100, Z_ue = Y_eidolon * 100)
    UnrealStandard = 0     UMETA(DisplayName = "Unreal Standard (Centimeters, Z-up)"),
    // Direct meters: X = x, Y = z, Z = y
    DirectMeters = 1       UMETA(DisplayName = "Direct Meters (Z-up)"),
    // Direct centimeters: X = x * 100, Y = z * 100, Z = y * 100
    DirectCentimeters = 2  UMETA(DisplayName = "Direct Centimeters (Z-up)")
};

/**
 * Replicated entity transform in Unreal Engine coordinate space.
 */
USTRUCT(BlueprintType)
struct FEidolonTransform
{
    GENERATED_BODY()

    UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "Eidolon")
    FVector Position = FVector::ZeroVector;

    UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "Eidolon")
    FRotator Rotation = FRotator::ZeroRotator;

    UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "Eidolon")
    FVector Velocity = FVector::ZeroVector;

    UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "Eidolon")
    uint8 Flags = 0;
};

/**
 * Hierarchical global coordinate for continental and planetary scale.
 */
USTRUCT(BlueprintType)
struct FEidolonGlobalCoord
{
    GENERATED_BODY()

    UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "Eidolon")
    int32 SectorX = 0;

    UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "Eidolon")
    int32 SectorZ = 0;

    UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "Eidolon")
    FVector LocalOffset = FVector::ZeroVector;
};

/**
 * High-speed vehicle kinematic input descriptor.
 */
USTRUCT(BlueprintType)
struct FEidolonVehicleIntent
{
    GENERATED_BODY()

    UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "Eidolon")
    uint8 VehicleType = 0;

    UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "Eidolon")
    uint8 Throttle = 0;

    UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "Eidolon")
    int8 Steering = 0;

    UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "Eidolon")
    int8 Pitch = 0;

    UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "Eidolon")
    int8 Roll = 0;
};

/**
 * Utility functions for coordinate transformations between Eidolon and Unreal.
 */
struct FEidolonCoordinateConversion
{
    static FVector ToUnrealPosition(float x, float y, float z, EEidolonCoordinateConvention convention)
    {
        switch (convention)
        {
            case EEidolonCoordinateConvention::UnrealStandard:
                // Eidolon: +Z forward, +X right, +Y up (meters)
                // Unreal:  +X forward, +Y right, +Z up (centimeters)
                return FVector(z * 100.0f, x * 100.0f, y * 100.0f);

            case EEidolonCoordinateConvention::DirectCentimeters:
                return FVector(x * 100.0f, z * 100.0f, y * 100.0f);

            case EEidolonCoordinateConvention::DirectMeters:
            default:
                return FVector(x, z, y);
        }
    }

    static void ToEidolonPosition(const FVector& pos, float& outX, float& outY, float& outZ, EEidolonCoordinateConvention convention)
    {
        switch (convention)
        {
            case EEidolonCoordinateConvention::UnrealStandard:
                outX = pos.Y * 0.01f;
                outY = pos.Z * 0.01f;
                outZ = pos.X * 0.01f;
                break;

            case EEidolonCoordinateConvention::DirectCentimeters:
                outX = pos.X * 0.01f;
                outY = pos.Z * 0.01f;
                outZ = pos.Y * 0.01f;
                break;

            case EEidolonCoordinateConvention::DirectMeters:
            default:
                outX = pos.X;
                outY = pos.Z;
                outZ = pos.Y;
                break;
        }
    }

    static FRotator ToUnrealRotation(float yawDeg)
    {
        return FRotator(0.0f, yawDeg, 0.0f);
    }

    static float ToEidolonYaw(const FRotator& rot)
    {
        return FMath::Fmod(rot.Yaw, 360.0f);
    }
};
