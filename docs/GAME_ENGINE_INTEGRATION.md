# Game Engine Integration Guide: Godot, Unreal Engine 5, Unity & Custom Engines

This guide details how to integrate `eidolon` into modern commercial and custom game engines.

`eidolon` provides a high-performance, unmanaged C ABI (`eidolon-ffi`) and ready-to-use language bindings, allowing any engine running C#, C++, C, or native scripts to connect directly to authoritative Eidolon MMO servers with sub-1.2 KB/s wire consumption, zero dynamic allocations during simulation, and smooth 60/120/144 FPS client-side dead reckoning extrapolation.

---

## Architecture Overview

```
                      +---------------------------------------+
                      |   Authoritative Eidolon MMO Server    |
                      |   (20 Hz Tick, Spatial AoI, WAL)      |
                      +---------------------------------------+
                                          ^
                                          | UDP (<1.2 KB/s)
                                          v
                      +---------------------------------------+
                      |        eidolon-client (Rust)          |
                      |  - Intent Streaming                   |
                      |  - 44-bit AoI State Reconstruction    |
                      |  - Continuous Dead Reckoning Extrap.  |
                      +---------------------------------------+
                                          ^
                                          | Native Call
                                          v
                      +---------------------------------------+
                      |         eidolon-ffi (C ABI)           |
                      |  - include/eidolon.h                  |
                      |  - libeidolon.so / .dylib / .dll      |
                      +---------------------------------------+
                           |                              |
            +--------------+---------------+              |
            |                              |              |
            v                              v              v
  +--------------------+        +--------------------+  +--------------------+
  |    Unity Engine    |        |  Godot Engine 4    |  |  Unreal Engine 5   |
  |  (C# P/Invoke)     |        |  (C# / GDExtension)|  |  (C++ Subsystem)   |
  +--------------------+        +--------------------+  +--------------------+
```

### Component Breakdown
1. **`crates/eidolon-client`**: Native pure Rust client library handling non-blocking UDP sockets, cryptographic handshake, reliable packet ordering, AoI transform reconstruction, and continuous dead reckoning extrapolation.
2. **`crates/eidolon-ffi`**: Unmanaged ANSI C99-compatible dynamic/static library exporting `extern "C"` functions with panic boundary protection (`catch_unwind`) and zero external dependencies.
3. **`include/eidolon.h`**: Standard C99 / C++ header definition with strict fixed-width types (`int32_t`, `uint32_t`, `float`).
4. **`bindings/csharp/EidolonClient.cs`**: Pure C# wrapper with P/Invoke, typed events, and `IDisposable` memory management for Unity and Godot (.NET).
5. **`bindings/cpp/EidolonClient.hpp`**: Modern C++17 RAII wrapper designed for Unreal Engine 5 and custom C++ game engines.

---

## 1. Building the Native Libraries

Compile the `eidolon-ffi` crate in release mode for your target platform:

```bash
# Build optimized release binary
cargo build --release -p eidolon-ffi
```

The resulting native libraries are produced in `target/release/`:
- **macOS**: `target/release/libeidolon.dylib`
- **Linux**: `target/release/libeidolon.so`
- **Windows**: `target/release/eidolon.dll` and `target/release/eidolon.lib`

---

## 2. Godot 4 Integration Guide

Godot 4 supports both C# (.NET) and C++ (GDExtension).

### Option A: Godot 4 (.NET / C#)
This is the recommended route for Godot 4 .NET projects.

1. **Copy Native Library**:
   Place `libeidolon.so` (Linux), `libeidolon.dylib` (macOS), or `eidolon.dll` (Windows) into your Godot project root:
   ```text
   my-godot-game/
   ├── addons/
   │   └── eidolon/
   │       ├── libeidolon.dylib (or .so / .dll)
   │       └── EidolonClient.cs
   ```
2. **Copy C# Binding**:
   Copy `bindings/csharp/EidolonClient.cs` into `addons/eidolon/`.
3. **Attach Network Controller**:
   Create a `NetworkManager.cs` node attached to your root scene:

```csharp
using Godot;
using System;
using System.Collections.Generic;
using Eidolon;

public partial class NetworkManager : Node
{
    private EidolonClient _client;
    private readonly Dictionary<uint, Node3D> _spawnedEntities = new Dictionary<uint, Node3D>();

    [Export] public PackedScene EntityPrefab;

    public override void _Ready()
    {
        // Initialize client configuration
        var config = new ClientConfig
        {
            ServerAddress = "127.0.0.1:7777",
            AccountId = 1001,
            SessionTicket = 0x12345678,
            TimeoutSeconds = 5.0f
        };

        _client = new EidolonClient(config);

        // Register event handlers
        _client.OnConnected += (sessionId, epoch) => {
            GD.Print($"[Godot] Connected to Eidolon MMO Server! Session: {sessionId}");
        };

        _client.OnEntitySpawned += (entityId, entityType, transform) => {
            GD.Print($"[Godot] Spawning Entity {entityId} at ({transform.X}, {transform.Y}, {transform.Z})");
            CallDeferred(nameof(SpawnEntityNode), entityId, transform.X, transform.Y, transform.Z);
        };

        _client.OnEntityDespawned += (entityId) => {
            CallDeferred(nameof(DespawnEntityNode), entityId);
        };

        _client.Connect();
    }

    public override void _PhysicsProcess(double delta)
    {
        if (_client == null || !_client.IsConnected) return;

        // 1. Poll incoming network events and AoI updates
        _client.PollEvents();

        // 2. Stream player movement intent
        Vector2 inputDir = Input.GetVector("ui_left", "ui_right", "ui_up", "ui_down");
        float vx = inputDir.X * 5.0f;
        float vz = inputDir.Y * 5.0f;
        _client.SendMovementIntent(vx, vz, 0.0f, inputDir != Vector2.Zero ? (byte)1 : (byte)0);

        // 3. Smooth dead reckoning extrapolation at native render FPS
        foreach (var kvp in _spawnedEntities)
        {
            uint eid = kvp.Key;
            Node3D node = kvp.Value;

            if (_client.ExtrapolateEntity(eid, (float)delta, out EidolonTransform tf))
            {
                node.GlobalPosition = new Vector3(tf.X, tf.Y, tf.Z);
                node.RotationDegrees = new Vector3(0.0f, tf.YawDegrees, 0.0f);
            }
        }
    }

    private void SpawnEntityNode(uint entityId, float x, float y, float z)
    {
        if (EntityPrefab == null) return;
        var instance = EntityPrefab.Instantiate<Node3D>();
        instance.GlobalPosition = new Vector3(x, y, z);
        AddChild(instance);
        _spawnedEntities[entityId] = instance;
    }

    private void DespawnEntityNode(uint entityId)
    {
        if (_spawnedEntities.TryGetValue(entityId, out var node))
        {
            node.QueueFree();
            _spawnedEntities.Remove(entityId);
        }
    }

    public override void _ExitTree()
    {
        _client?.Dispose();
        _client = null;
    }
}
```

---

## 3. Unreal Engine 5 Integration Guide

Unreal Engine uses C++ modules for third-party native integration.

### Step 1: Add Third-Party Module
In your UE5 project directory:
```text
MyGame/
├── Source/
│   └── ThirdParty/
│       └── Eidolon/
│           ├── Eidolon.Build.cs
│           ├── include/
│           │   ├── eidolon.h
│           │   └── EidolonClient.hpp
│           └── lib/
│               ├── Win64/eidolon.lib
│               ├── Mac/libeidolon.dylib
│               └── Linux/libeidolon.so
```

### Step 2: Configure `Eidolon.Build.cs`
```csharp
using System.IO;
using UnrealBuildTool;

public class Eidolon : ModuleRules
{
    public Eidolon(ReadOnlyTargetRules Target) : base(Target)
    {
        Type = ModuleType.External;

        PublicIncludePaths.Add(Path.Combine(ModuleDirectory, "include"));

        if (Target.Platform == UnrealTargetPlatform.Win64)
        {
            PublicAdditionalLibraries.Add(Path.Combine(ModuleDirectory, "lib", "Win64", "eidolon.lib"));
            PublicDelayLoadDLLs.Add("eidolon.dll");
            RuntimeDependencies.Add(Path.Combine("$(BinaryOutputDir)", "eidolon.dll"),
                                    Path.Combine(ModuleDirectory, "lib", "Win64", "eidolon.dll"));
        }
        else if (Target.Platform == UnrealTargetPlatform.Mac)
        {
            PublicAdditionalLibraries.Add(Path.Combine(ModuleDirectory, "lib", "Mac", "libeidolon.dylib"));
            RuntimeDependencies.Add(Path.Combine(ModuleDirectory, "lib", "Mac", "libeidolon.dylib"));
        }
        else if (Target.Platform == UnrealTargetPlatform.Linux)
        {
            PublicAdditionalLibraries.Add(Path.Combine(ModuleDirectory, "lib", "Linux", "libeidolon.so"));
            RuntimeDependencies.Add(Path.Combine(ModuleDirectory, "lib", "Linux", "libeidolon.so"));
        }
    }
}
```

Add `"Eidolon"` to `PublicDependencyModuleNames` in your game module's `.Build.cs`.

### Step 3: Implement `UEidolonSubsystem`
Create an engine or game instance subsystem:

```cpp
#pragma once

#include "CoreMinimal.h"
#include "Subsystems/GameInstanceSubsystem.h"
#include "EidolonClient.hpp"
#include "EidolonSubsystem.generated.h"

UCLASS()
class MYGAME_API UEidolonSubsystem : public UGameInstanceSubsystem, public FTickableGameObject
{
    GENERATED_BODY()

public:
    virtual void Initialize(FSubsystemCollectionBase& Collection) override;
    virtual void Deinitialize() override;

    // FTickableGameObject interface
    virtual void Tick(float DeltaTime) override;
    virtual TStatId GetStatId() const override { RETURN_QUICK_DECLARE_CYCLE_STAT(UEidolonSubsystem, STATGROUP_Tickables); }
    virtual bool IsTickable() const override { return Client.has_value() && Client->IsConnected(); }

    void ConnectToServer(const FString& Host, uint16 Port, uint64 AccountId, uint64 SessionTicket);
    void SendIntent(float Vx, float Vz, float Yaw, uint8 Flags);

private:
    std::optional<eidolon::EidolonClient> Client;
};
```

Implementation in `EidolonSubsystem.cpp`:
```cpp
#include "EidolonSubsystem.h"

void UEidolonSubsystem::Initialize(FSubsystemCollectionBase& Collection)
{
    Super::Initialize(Collection);
}

void UEidolonSubsystem::Deinitialize()
{
    if (Client.has_value()) {
        Client->Disconnect();
        Client.reset();
    }
    Super::Deinitialize();
}

void UEidolonSubsystem::ConnectToServer(const FString& Host, uint16 Port, uint64 AccountId, uint64 SessionTicket)
{
    eidolon::ClientConfig Config;
    Config.ServerAddress = std::string(TCHAR_TO_UTF8(*FString::Printf(TEXT("%s:%d"), *Host, Port)));
    Config.AccountId = AccountId;
    Config.SessionTicket = SessionTicket;
    Config.TimeoutSeconds = 5.0f;

    Client.emplace(Config);

    Client->SetConnectedCallback([](uint32_t SessionId, uint32_t Epoch) {
        UE_LOG(LogTemp, Log, TEXT("[Eidolon UE5] Connected! Session: %u, Epoch: %u"), SessionId, Epoch);
    });

    Client->SetEntitySpawnedCallback([](uint32_t EntityId, uint8_t EntityType, const EidolonTransform& Transform) {
        UE_LOG(LogTemp, Log, TEXT("[Eidolon UE5] Entity %u Spawned at (%.2f, %.2f, %.2f)"), EntityId, Transform.x, Transform.y, Transform.z);
    });

    if (!Client->Connect()) {
        UE_LOG(LogTemp, Error, TEXT("[Eidolon UE5] Failed to initiate UDP connection"));
    }
}

void UEidolonSubsystem::Tick(float DeltaTime)
{
    if (!Client.has_value() || !Client->IsConnected()) return;

    // 1. Poll incoming events from native Rust client
    Client->PollEvents();

    // 2. Extrapolate visible entities at high framerate
    std::vector<uint32_t> EntityIds = Client->GetVisibleEntities();
    for (uint32_t Eid : EntityIds) {
        EidolonTransform OutTf;
        if (Client->ExtrapolateEntity(Eid, DeltaTime, OutTf)) {
            // Update actor position in Unreal world:
            // FVector Location(OutTf.x * 100.0f, OutTf.z * 100.0f, OutTf.y * 100.0f); // Convert to Unreal cm
            // FRotator Rotation(0.0f, OutTf.yaw_degrees, 0.0f);
        }
    }
}

void UEidolonSubsystem::SendIntent(float Vx, float Vz, float Yaw, uint8 Flags)
{
    if (Client.has_value()) {
        Client->SendMovementIntent(Vx, Vz, Yaw, Flags);
    }
}
```

---

## 4. Unity Integration Guide (2021+, 2022+, Unity 6)

### Step 1: Copy Native Libraries and Script
Place native library files inside `Assets/Plugins/Eidolon/`:
```text
Assets/
└── Plugins/
    └── Eidolon/
        ├── x86_64/
        │   ├── eidolon.dll         (Windows x64)
        │   └── libeidolon.so       (Linux x64)
        └── macOS/
            └── libeidolon.dylib    (macOS Universal/ARM64)
```
In Unity Inspector, select each plugin file and configure the target platform (Standalone Windows, Linux, macOS).

Copy `bindings/csharp/EidolonClient.cs` into `Assets/Scripts/Networking/`.

### Step 2: Create Unity `NetworkManager.cs`
```csharp
using System.Collections.Generic;
using UnityEngine;
using Eidolon;

public class EidolonNetworkManager : MonoBehaviour
{
    private EidolonClient _client;
    private readonly Dictionary<uint, GameObject> _actors = new Dictionary<uint, GameObject>();

    [SerializeField] private GameObject playerPrefab;
    [SerializeField] private GameObject monsterPrefab;

    void Start()
    {
        var config = new ClientConfig
        {
            ServerAddress = "127.0.0.1:7777",
            AccountId = 1001,
            SessionTicket = 0xDEADBEEF,
            TimeoutSeconds = 5.0f
        };

        _client = new EidolonClient(config);

        _client.OnConnected += (sessionId, epoch) => {
            Debug.Log($"[Unity] Connected to Eidolon server! Session: {sessionId}");
        };

        _client.OnEntitySpawned += (eid, type, tf) => {
            GameObject prefab = type == 0 ? playerPrefab : monsterPrefab;
            if (prefab != null) {
                var go = Instantiate(prefab, new Vector3(tf.X, tf.Y, tf.Z), Quaternion.Euler(0, tf.YawDegrees, 0));
                _actors[eid] = go;
            }
        };

        _client.OnEntityDespawned += (eid) => {
            if (_actors.TryGetValue(eid, out var go)) {
                Destroy(go);
                _actors.Remove(eid);
            }
        };

        _client.Connect();
    }

    void Update()
    {
        if (_client == null || !_client.IsConnected) return;

        // 1. Process server packets
        _client.PollEvents();

        // 2. Send input intent
        float h = Input.GetAxisRaw("Horizontal");
        float v = Input.GetAxisRaw("Vertical");
        float vx = h * 6.0f;
        float vz = v * 6.0f;
        byte flags = (h != 0 || v != 0) ? (byte)1 : (byte)0;
        _client.SendMovementIntent(vx, vz, 0.0f, flags);

        // 3. Extrapolate remote actors at 60/120/144 FPS
        foreach (var kvp in _actors)
        {
            uint eid = kvp.Key;
            GameObject go = kvp.Value;

            if (_client.ExtrapolateEntity(eid, Time.deltaTime, out EidolonTransform tf))
            {
                go.transform.position = new Vector3(tf.X, tf.Y, tf.Z);
                go.transform.rotation = Quaternion.Euler(0, tf.YawDegrees, 0);
            }
        }
    }

    void OnDestroy()
    {
        _client?.Dispose();
        _client = null;
    }
}
```

---

## 5. Custom & In-House C / C++ Game Engines

For custom game engines, include `include/eidolon.h` (C) or `bindings/cpp/EidolonClient.hpp` (C++):

```cpp
#include "EidolonClient.hpp"
#include <iostream>
#include <thread>
#include <chrono>

int main() {
    eidolon::ClientConfig config;
    config.ServerAddress = "127.0.0.1:7777";
    config.AccountId = 42;
    config.SessionTicket = 0xCAFEBABE;
    config.TimeoutSeconds = 3.0f;

    eidolon::EidolonClient client(config);

    client.SetConnectedCallback([](uint32_t session_id, uint32_t epoch) {
        std::cout << "Custom Engine: Connected! Session " << session_id << std::endl;
    });

    client.SetEntitySpawnedCallback([](uint32_t id, uint8_t type, const EidolonTransform& tf) {
        std::cout << "Custom Engine: Entity " << id << " spawned at ("
                  << tf.x << ", " << tf.y << ", " << tf.z << ")" << std::endl;
    });

    if (!client.Connect()) {
        std::cerr << "Failed to connect to server" << std::endl;
        return 1;
    }

    // Engine loop
    while (client.IsConnected()) {
        // Poll incoming network packets
        client.PollEvents();

        // Send WASD movement vector
        client.SendMovementIntent(1.5f, 0.0f, 90.0f, 1);

        // Extrapolate visible entities at render tick
        for (uint32_t eid : client.GetVisibleEntities()) {
            EidolonTransform tf;
            if (client.ExtrapolateEntity(eid, 0.016f, tf)) {
                // Pass tf.x, tf.y, tf.z to render matrix
            }
        }

        std::this_thread::sleep_for(std::chrono::milliseconds(16));
    }

    return 0;
}
```

---

## 6. Dead Reckoning & Extrapolation Invariants

- **Network Frequency**: 20 Hz fixed tick rate (50ms between server packet emissions).
- **Client Frame Rate**: Extrapolation runs seamlessly at 60 FPS (16.6ms), 120 FPS (8.3ms), or 144 FPS (6.9ms).
- **Extrapolation Math**:
  $$\vec{P}_{render} = \vec{P}_{base} + \vec{V} \cdot \Delta t$$
  The client uses server-validated velocity intent ($\vec{V}$) and yaw angle ($\theta$) to smoothly advance remote entities between 20 Hz state packet arrivals without visual snapping or rubber-banding.
- **Quantization**: Coordinates are packed into 16-bit cell-relative integers and 8-bit yaw values over UDP, keeping bandwidth under 1.2 KB/s per client.
