// EidolonSubsystem.h: GameInstance Subsystem for Blueprint & C++ MMO Integration

#pragma once

#include "CoreMinimal.h"
#include "Subsystems/GameInstanceSubsystem.h"
#include "Containers/Ticker.h"
#include "EidolonTypes.h"
#include "EidolonSubsystem.generated.h"

// Dynamic Multicast Delegates for Blueprints
DECLARE_DYNAMIC_MULTICAST_DELEGATE_OneParam(FOnEidolonConnected, int64, AccountId);
DECLARE_DYNAMIC_MULTICAST_DELEGATE_OneParam(FOnEidolonDisconnected, int32, ReasonCode);
DECLARE_DYNAMIC_MULTICAST_DELEGATE_FourParams(FOnEidolonEntitySpawned, int64, EntityId, uint8, EntityType, FVector, Position, FRotator, Rotation);
DECLARE_DYNAMIC_MULTICAST_DELEGATE_FourParams(FOnEidolonEntityUpdated, int64, EntityId, FVector, Position, FRotator, Rotation, FVector, Velocity);
DECLARE_DYNAMIC_MULTICAST_DELEGATE_OneParam(FOnEidolonEntityDespawned, int64, EntityId);
DECLARE_DYNAMIC_MULTICAST_DELEGATE_ThreeParams(FOnEidolonChatMessage, uint8, Channel, int64, SenderId, const FString&, Message);
DECLARE_DYNAMIC_MULTICAST_DELEGATE_OneParam(FOnEidolonTimeDilationChanged, float, TimeDilation);

// Forward declaration of raw C handle
struct EidolonClientHandle;

/**
 * Universal GameInstance Subsystem managing Eidolon client lifecycle and events.
 * Accessible anywhere in Unreal Engine via Blueprints or C++:
 * GetGameInstance()->GetSubsystem<UEidolonSubsystem>()
 */
UCLASS()
class EIDOLONCLIENT_API UEidolonSubsystem : public UGameInstanceSubsystem
{
    GENERATED_BODY()

public:
    UEidolonSubsystem();

    virtual void Initialize(FSubsystemCollectionBase& Collection) override;
    virtual void Deinitialize() override;

    // Blueprint Event Dispatchers
    UPROPERTY(BlueprintAssignable, Category = "Eidolon|Events")
    FOnEidolonConnected OnConnected;

    UPROPERTY(BlueprintAssignable, Category = "Eidolon|Events")
    FOnEidolonDisconnected OnDisconnected;

    UPROPERTY(BlueprintAssignable, Category = "Eidolon|Events")
    FOnEidolonEntitySpawned OnEntitySpawned;

    UPROPERTY(BlueprintAssignable, Category = "Eidolon|Events")
    FOnEidolonEntityUpdated OnEntityUpdated;

    UPROPERTY(BlueprintAssignable, Category = "Eidolon|Events")
    FOnEidolonEntityDespawned OnEntityDespawned;

    UPROPERTY(BlueprintAssignable, Category = "Eidolon|Events")
    FOnEidolonChatMessage OnChatMessageReceived;

    UPROPERTY(BlueprintAssignable, Category = "Eidolon|Events")
    FOnEidolonTimeDilationChanged OnTimeDilationChanged;

    // Blueprint Callable Interface
    UFUNCTION(BlueprintCallable, Category = "Eidolon|Connection")
    bool Connect(FString Host, int32 Port, int64 AccountId);

    UFUNCTION(BlueprintCallable, Category = "Eidolon|Connection")
    void Disconnect();

    UFUNCTION(BlueprintPure, Category = "Eidolon|Connection")
    bool IsConnected() const;

    UFUNCTION(BlueprintCallable, Category = "Eidolon|Movement")
    bool SendMovementIntent(FVector Velocity, float YawDeg, uint8 Flags);

    UFUNCTION(BlueprintCallable, Category = "Eidolon|Combat")
    bool CastAbility(int64 TargetEntityId, int32 AbilityId);

    UFUNCTION(BlueprintCallable, Category = "Eidolon|Inventory")
    bool EquipItem(uint8 InventorySlot, uint8 EquipSlot);

    UFUNCTION(BlueprintCallable, Category = "Eidolon|Chat")
    bool SendChatMessage(uint8 Channel, int64 TargetId, const FString& Text);

    UFUNCTION(BlueprintCallable, Category = "Eidolon|Vehicles")
    bool SendVehicleIntent(const FEidolonVehicleIntent& Intent);

    UFUNCTION(BlueprintCallable, Category = "Eidolon|Terrain")
    bool QueryTerrainHeight(int32 SectorX, int32 SectorZ, float LocalX, float LocalZ, float& OutElevation);

private:
    bool OnTick(float DeltaTime);

    EidolonClientHandle* ClientHandle;
    FTSTicker::FDelegateHandle TickerHandle;
};
