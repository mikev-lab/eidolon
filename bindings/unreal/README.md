# Eidolon Unreal Engine 5 Plugin: Turnkey MMO Integration

The Eidolon Unreal Engine 5 Plugin delivers native, high-performance MMO client networking for UE 5.0 through 5.4+, featuring Blueprint-accessible GameInstance subsystems, automatic frame polling via `FTSTicker`, Project Settings integration, and actor replication components with sub-centimeter dead reckoning smoothing.

---

## 1. Quickstart in 3 Minutes

### Step 1: Install the Plugin
Copy the `bindings/unreal/` directory into your Unreal Engine project's `Plugins/EidolonClient/` folder:

```text
MyUnrealGame/
├── Plugins/
│   └── EidolonClient/
│       ├── EidolonClient.uplugin
│       └── Source/
```

Place the compiled native binary (`eidolon.dll` on Windows, `libeidolon.a` on macOS/Linux) into your project's `Binaries/` or `Plugins/EidolonClient/Binaries/` folder.

### Step 2: Configure Project Settings
In the Unreal Editor:
1. Open **Edit -> Project Settings**.
2. Scroll to the **Game** category and click **Eidolon Client**.
3. Configure your server endpoints:
   - **Server Host:** `127.0.0.1` (or production host)
   - **Server Port:** `7777`
   - **Density Profile:** Standard MMO (or Mobile / Massive Fleet)
   - **Coordinate Convention:** Unreal Standard (Centimeters, Z-up)

### Step 3: Access via Blueprints or C++

#### In Blueprints:
1. In your Game Mode or Player Controller, call **Get Game Instance -> Get Eidolon Subsystem**.
2. Call **Connect** with your Host and Account ID.
3. Bind to **On Entity Spawned** and **On Entity Updated** events.

#### In C++:
```cpp
#include "EidolonSubsystem.h"

void AMyPlayerController::BeginPlay()
{
    Super::BeginPlay();

    UEidolonSubsystem* Eidolon = GetGameInstance()->GetSubsystem<UEidolonSubsystem>();
    if (Eidolon != nullptr)
    {
        Eidolon->OnConnected.AddDynamic(this, &AMyPlayerController::HandleConnected);
        Eidolon->OnEntitySpawned.AddDynamic(this, &AMyPlayerController::HandleEntitySpawned);

        Eidolon->Connect(TEXT("127.0.0.1"), 7777, 1001);
    }
}

void AMyPlayerController::HandleConnected(int64 AccountId)
{
    UE_LOG(LogTemp, Log, TEXT("Connected to Eidolon MMO with Account: %lld"), AccountId);
}

void AMyPlayerController::HandleEntitySpawned(int64 EntityId, uint8 EntityType, FVector Position, FRotator Rotation)
{
    FActorSpawnParameters SpawnParams;
    ACharacter* ReplicatedChar = GetWorld()->SpawnActor<ACharacter>(CharacterClass, Position, Rotation, SpawnParams);

    if (ReplicatedChar != nullptr)
    {
        UEidolonEntityComponent* Comp = ReplicatedChar->FindComponentByClass<UEidolonEntityComponent>();
        if (Comp != nullptr)
        {
            Comp->InitializeEntity(EntityId, EntityType, Position, Rotation);
        }
    }
}
```

---

## 2. Remote Entity Dead Reckoning (`UEidolonEntityComponent`)

Attach `UEidolonEntityComponent` to your remote Character or Pawn Blueprints:
- Automatically interpolates `ActorLocation` and `ActorRotation` in `TickComponent` at client framerates (60/120/144/240 Hz).
- Performs forward dead reckoning if server packets arrive irregularly.
- Automatically handles coordinate conversion between Eidolon's right-handed meter coordinates and Unreal's left-handed centimeter coordinates ($X_{\text{UE}} = Z_{\text{E}} \times 100, Y_{\text{UE}} = X_{\text{E}} \times 100, Z_{\text{UE}} = Y_{\text{E}} \times 100$).
