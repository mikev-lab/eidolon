// EidolonSettings.h: Unreal Engine Project Settings Integration

#pragma once

#include "CoreMinimal.h"
#include "Engine/DeveloperSettings.h"
#include "EidolonTypes.h"
#include "EidolonSettings.generated.h"

/**
 * Global configuration settings for the Eidolon Client Plugin.
 * Visible in Unreal Editor: Project Settings -> Game -> Eidolon Client.
 */
UCLASS(config = Game, defaultconfig, meta = (DisplayName = "Eidolon Client"))
class EIDOLONCLIENT_API UEidolonSettings : public UDeveloperSettings
{
    GENERATED_BODY()

public:
    UEidolonSettings();

    // Authoritative server hostname or IP address
    UPROPERTY(config, EditAnywhere, Category = "Connection", meta = (DisplayName = "Server Host"))
    FString ServerHost = TEXT("127.0.0.1");

    // Authoritative server UDP port
    UPROPERTY(config, EditAnywhere, Category = "Connection", meta = (DisplayName = "Server Port"))
    int32 ServerPort = 7777;

    // Default account identifier for development test sessions
    UPROPERTY(config, EditAnywhere, Category = "Connection", meta = (DisplayName = "Default Account ID"))
    int64 DefaultAccountId = 1001;

    // Bandwidth density profile
    UPROPERTY(config, EditAnywhere, Category = "Bandwidth", meta = (DisplayName = "Density Profile"))
    EEidolonDensityProfile DensityProfile = EEidolonDensityProfile::StandardMMO;

    // Coordinate mapping convention between Eidolon world space and Unreal
    UPROPERTY(config, EditAnywhere, Category = "Coordinates", meta = (DisplayName = "Coordinate Convention"))
    EEidolonCoordinateConvention CoordinateConvention = EEidolonCoordinateConvention::UnrealStandard;

    // Interpolation delay in seconds (e.g. 0.05s = 50ms)
    UPROPERTY(config, EditAnywhere, Category = "Dead Reckoning", meta = (DisplayName = "Interpolation Delay (Seconds)"))
    float InterpolationDelay = 0.05f;

    // Maximum distance in centimeters before an instant snap is triggered instead of smoothing
    UPROPERTY(config, EditAnywhere, Category = "Dead Reckoning", meta = (DisplayName = "Snap Distance Threshold (cm)"))
    float SnapDistanceThreshold = 1000.0f;

    virtual FName GetCategoryName() const override
    {
        return FName(TEXT("Game"));
    }
};
