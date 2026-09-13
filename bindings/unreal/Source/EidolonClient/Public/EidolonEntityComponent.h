// EidolonEntityComponent.h: Actor Component for Smooth Remote Entity Replication in UE5

#pragma once

#include "CoreMinimal.h"
#include "Components/ActorComponent.h"
#include "EidolonTypes.h"
#include "EidolonEntityComponent.generated.h"

/**
 * Attached to replicated remote Actor or Character blueprints to smoothly interpolate
 * network transforms at client display refresh rates (60/120/144/240 Hz).
 */
UCLASS(ClassGroup = (Eidolon), meta = (BlueprintSpawnableComponent))
class EIDOLONCLIENT_API UEidolonEntityComponent : public UActorComponent
{
    GENERATED_BODY()

public:
    UEidolonEntityComponent();

    virtual void TickComponent(float DeltaTime, ELevelTick TickType, FActorComponentTickFunction* ThisTickFunction) override;

    UFUNCTION(BlueprintCallable, Category = "Eidolon|Replication")
    void InitializeEntity(int64 InEntityId, uint8 InEntityType, FVector InitialPosition, FRotator InitialRotation);

    UFUNCTION(BlueprintCallable, Category = "Eidolon|Replication")
    void UpdateTransform(FVector NewPosition, FRotator NewRotation, FVector NewVelocity);

    UPROPERTY(VisibleAnywhere, BlueprintReadOnly, Category = "Eidolon|State")
    int64 EntityId = 0;

    UPROPERTY(VisibleAnywhere, BlueprintReadOnly, Category = "Eidolon|State")
    uint8 EntityType = 0;

    UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "Eidolon|Smoothing")
    float PositionSmoothingSpeed = 15.0f;

    UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "Eidolon|Smoothing")
    float RotationSmoothingSpeed = 12.0f;

    UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "Eidolon|Smoothing")
    float SnapDistanceThreshold = 1000.0f; // 10 meters in cm

private:
    FVector TargetPosition = FVector::ZeroVector;
    FRotator TargetRotation = FRotator::ZeroRotator;
    FVector Velocity = FVector::ZeroVector;
    float TimeSinceLastUpdate = 0.0f;
    bool bIsInitialized = false;
};
