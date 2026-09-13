// EidolonEntityComponent.cpp: Implementation of UE5 Entity Replication Component

#include "EidolonEntityComponent.h"
#include "GameFramework/Actor.h"

UEidolonEntityComponent::UEidolonEntityComponent()
{
    PrimaryComponentTick.bCanEverTick = true;
    PrimaryComponentTick.bStartWithTickEnabled = true;
}

void UEidolonEntityComponent::InitializeEntity(int64 InEntityId, uint8 InEntityType, FVector InitialPosition, FRotator InitialRotation)
{
    EntityId = InEntityId;
    EntityType = InEntityType;
    TargetPosition = InitialPosition;
    TargetRotation = InitialRotation;
    TimeSinceLastUpdate = 0.0f;
    bIsInitialized = true;

    AActor* Owner = GetOwner();
    if (Owner != nullptr)
    {
        Owner->SetActorLocationAndRotation(InitialPosition, InitialRotation);
    }
}

void UEidolonEntityComponent::UpdateTransform(FVector NewPosition, FRotator NewRotation, FVector NewVelocity)
{
    AActor* Owner = GetOwner();
    if (Owner != nullptr)
    {
        float DistSq = FVector::DistSquared(Owner->GetActorLocation(), NewPosition);
        if (DistSq > SnapDistanceThreshold * SnapDistanceThreshold || !bIsInitialized)
        {
            Owner->SetActorLocationAndRotation(NewPosition, NewRotation);
            TargetPosition = NewPosition;
            TargetRotation = NewRotation;
        }
        else
        {
            TargetPosition = NewPosition;
            TargetRotation = NewRotation;
        }
    }

    Velocity = NewVelocity;
    TimeSinceLastUpdate = 0.0f;
    bIsInitialized = true;
}

void UEidolonEntityComponent::TickComponent(float DeltaTime, ELevelTick TickType, FActorComponentTickFunction* ThisTickFunction)
{
    Super::TickComponent(DeltaTime, TickType, ThisTickFunction);

    if (!bIsInitialized)
    {
        return;
    }

    AActor* Owner = GetOwner();
    if (Owner == nullptr)
    {
        return;
    }

    TimeSinceLastUpdate += DeltaTime;

    FVector ProjectedTarget = TargetPosition;
    if (TimeSinceLastUpdate > 0.05f && Velocity.SizeSquared() > 1.0f)
    {
        float ExtraTime = FMath::Min(TimeSinceLastUpdate - 0.05f, 0.50f);
        ProjectedTarget += Velocity * ExtraTime;
    }

    float PosAlpha = 1.0f - FMath::Exp(-PositionSmoothingSpeed * DeltaTime);
    FVector SmoothPos = FMath::Lerp(Owner->GetActorLocation(), ProjectedTarget, PosAlpha);

    float RotAlpha = 1.0f - FMath::Exp(-RotationSmoothingSpeed * DeltaTime);
    FRotator SmoothRot = FMath::RInterpTo(Owner->GetActorRotation(), TargetRotation, DeltaTime, RotationSmoothingSpeed);

    Owner->SetActorLocationAndRotation(SmoothPos, SmoothRot);
}
