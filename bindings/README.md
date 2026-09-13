# Eidolon Universal Multi-Engine Client SDK

The Eidolon Client SDK provides official, turnkey, and high-performance client networking integrations for all major modern game engines:

- **Unity 2021+:** Official Unity Package Manager (UPM) package (`bindings/unity/`) with `EidolonNetworkManager` singleton, `EidolonEntityView` interpolation, `EidolonLocalPlayer` input sampler, and `EidolonConfig` ScriptableObjects.
- **Unreal Engine 5 (UE 5.0 - 5.4+):** Official Unreal Engine plugin (`bindings/unreal/`) with `UEidolonSubsystem` (`UGameInstanceSubsystem`), `UEidolonEntityComponent` (`UActorComponent`), Blueprint callable nodes, and Project Settings integration.
- **Godot 4 (.NET / C#):** Official Godot 4 addon (`bindings/godot/`) featuring `EidolonNetwork.cs` Autoload singleton, `EidolonEntityNode3D.cs` smooth replication, and signal routing.
- **Modern C++17:** Production-ready headers (`bindings/cpp/`) with RAII `Client`, `Interpolator` (Hermite spline / dead reckoning), `EntityManager`, and `ThreadedClient`.
- **C# (.NET Standard 2.1):** Raw P/Invoke wrappers and managed events (`bindings/csharp/EidolonClient.cs`).
- **C ABI (ANSI C99):** Unmanaged foreign function interface (`include/eidolon.h`).

---

## 1. Engine Compatibility & Coordinate Mapping Matrix

Each game engine uses different spatial axes and units. Eidolon provides zero-cost, automatic coordinate adapters for all conventions:

| Engine | Coordinate System | Up Axis | Forward Axis | Unit Scale | SDK Mapping Strategy |
| :--- | :--- | :---: | :---: | :---: | :--- |
| **Eidolon Engine Core** | Right-Handed | $+Y$ | $+Z$ | 1.0 m | Native canonical coordinate system |
| **Godot 4** | Right-Handed | $+Y$ | $-Z$ | 1.0 m | **Direct 1:1 Parity:** Identical coordinate representation |
| **Unity 2021+** | Left-Handed | $+Y$ | $+Z$ | 1.0 m | **Direct or InvertZ:** Automatic handedness parity in `EidolonCoordinateUtils` |
| **Unreal Engine 5** | Left-Handed | $+Z$ | $+X$ | 0.01 m (cm) | **Automatic Centimeter Scaling:** $X_{\text{UE}} = Z \times 100, Y_{\text{UE}} = X \times 100, Z_{\text{UE}} = Y \times 100$ |

---

## 2. Feature Support Across Engines

| Engine Capability | Unity | Unreal Engine 5 | Godot 4 | Modern C++17 | C ABI |
| :--- | :---: | :---: | :---: | :---: | :---: |
| **Sub-1.2 KB/s Intent Streaming** | Yes (`EidolonLocalPlayer`) | Yes (`SendMovementIntent`) | Yes (`SendIntent`) | Yes (`send_movement`) | Yes (`eidolon_client_send_intent`) |
| **60/120/144/240 FPS Dead Reckoning** | Yes (`EidolonEntityView`) | Yes (`UEidolonEntityComponent`) | Yes (`EidolonEntityNode3D`) | Yes (`Interpolator`) | Yes (`eidolon_client_extrapolate_entity`) |
| **Automatic Entity Prefab Spawning** | Yes (`_prefabMappings`) | Yes (Blueprint Events) | Yes (`DefaultEntityScene`) | Yes (`EntityManager`) | Yes (`eidolon_client_poll_events`) |
| **Inspector / Project Settings** | Yes (`EidolonConfig`) | Yes (`UEidolonSettings`) | Yes (`EidolonGodotConfig`) | Yes (`InterpolatorConfig`) | N/A |
| **Blueprint / Signal Integration** | UnityEvents | Dynamic Multicast | C# Signals | std::function | C Callbacks |
| **High-Speed Vehicle Kinematics** | Yes (`SendVehicleIntent`) | Yes (`SendVehicleIntent`) | Yes (`SendVehicleIntent`) | Yes (`send_vehicle_intent`) | Yes (`eidolon_client_send_vehicle_intent`) |
| **Planetary Coordinates (`GlobalCoord`)**| Yes (`EidolonGlobalCoord`) | Yes (`FEidolonGlobalCoord`) | Yes (`EidolonGlobalCoord`) | Yes (`get_global_coord`) | Yes (`eidolon_client_get_global_coord`) |
| **Macro HLOD Terrain Height Queries** | Yes (`QueryTerrainHeight`) | Yes (`QueryTerrainHeight`) | Yes (`QueryTerrainHeight`) | Yes (`query_terrain_height`)| Yes (`eidolon_client_query_terrain_height`) |
| **Player Housing & Freeform Building** | Yes (`PlaceStructure`) | Yes (C ABI) | Yes (C ABI) | Yes (`place_structure`) | Yes (`eidolon_client_place_structure`) |

---

## 3. Directory Layout

```text
bindings/
├── README.md                  # This overview and coordinate matrix
├── csharp/                    # Universal C# .NET Standard 2.1 P/Invoke
│   └── EidolonClient.cs
├── cpp/                       # Modern C++17 RAII SDK and utilities
│   ├── EidolonClient.hpp      # RAII client wrapper
│   ├── EidolonInterpolator.hpp# 60/120/144 FPS Hermite spline buffer
│   ├── EidolonEntityManager.hpp# Client entity registry & lifecycle
│   └── EidolonThreadedClient.hpp# Background network thread
├── unity/                     # Turnkey Unity Package (UPM / Drag-and-Drop)
│   ├── package.json           # UPM package manifest
│   ├── README.md              # 3-minute Unity quickstart
│   └── Runtime/
│       ├── Eidolon.Runtime.asmdef
│       ├── EidolonConfig.cs   # ScriptableObject configuration asset
│       ├── EidolonCoordinateUtils.cs # Coordinate conversions & angle smoothing
│       ├── EidolonEntityView.cs# Smooth remote entity replication component
│       ├── EidolonLocalPlayer.cs # Local input sampling & 20 Hz intent streaming
│       └── EidolonNetworkManager.cs # Central singleton manager
├── unreal/                    # Turnkey Unreal Engine 5 Plugin
│   ├── EidolonClient.uplugin  # UE5 plugin descriptor
│   ├── README.md              # 3-minute Unreal Engine 5 quickstart
│   └── Source/
│       └── EidolonClient/
│           ├── EidolonClient.Build.cs
│           ├── Public/
│           │   ├── EidolonEntityComponent.h # Actor replication component
│           │   ├── EidolonSettings.h  # Project Settings integration
│           │   ├── EidolonSubsystem.h # UGameInstanceSubsystem for Blueprints & C++
│           │   ├── EidolonTypes.h     # USTRUCTs for Blueprint nodes
│           │   └── IEidolonClientModule.h
│           └── Private/
│               ├── EidolonClientModule.cpp
│               ├── EidolonEntityComponent.cpp
│               ├── EidolonSettings.cpp
│               └── EidolonSubsystem.cpp
└── godot/                     # Turnkey Godot 4 (.NET / C#) Addon
    ├── plugin.cfg             # Godot addon descriptor
    ├── README.md              # 3-minute Godot 4 quickstart
    ├── EidolonEntityNode3D.cs # Smooth Node3D interpolation controller
    ├── EidolonGodotConfig.cs  # Resource configuration asset
    └── EidolonNetwork.cs      # Autoload / Node singleton with signals
```
