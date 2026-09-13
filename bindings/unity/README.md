# Eidolon Unity Client SDK: Turnkey MMO Integration

The Eidolon Unity Client SDK provides drag-and-drop MMO networking for Unity 2021+ (.NET Standard 2.1), featuring Inspector-configurable network management, automatic entity prefab spawning, and smooth 60/120/144+ FPS dead reckoning interpolation decoupled from 20 Hz server updates.

---

## 1. Quickstart in 3 Minutes

### Step 1: Install the Package
Add the package to your project's `Packages/manifest.json`:
```json
{
  "dependencies": {
    "com.eidolon.client": "file:../../bindings/unity"
  }
}
```
Or copy the `bindings/unity/` folder directly into your Unity project's `Assets/Plugins/Eidolon/` directory.

Ensure the native library (`libeidolon.dylib` on macOS, `eidolon.dll` on Windows, or `libeidolon.so` on Linux) is placed in `Assets/Plugins/Eidolon/Native/`.

### Step 2: Create a Client Configuration Asset
1. In the Unity Project window, right-click and select **Create -> Eidolon -> Client Configuration**.
2. Name the asset `EidolonConfig`.
3. In the Inspector, configure:
   - **Server Host:** `127.0.0.1` (or your server IP)
   - **Server Port:** `7777`
   - **Density Profile:** Standard MMO (or Mobile / Massive Fleet)
   - **Coordinate Convention:** DirectParity (or InvertZ)

### Step 3: Add `EidolonNetworkManager` to the Scene
1. Create an empty GameObject named `NetworkManager`.
2. Add the `EidolonNetworkManager` component.
3. Drag your `EidolonConfig` asset into the **Config** slot.
4. (Optional) Assign entity prefabs in the **Prefab Mappings** list.

### Step 4: Add `EidolonLocalPlayer` to Your Character
Attach `EidolonLocalPlayer` to your playable character GameObject. It will automatically sample WASD input and dispatch 20 Hz intent updates to the server.

---

## 2. Remote Entity Replication (`EidolonEntityView`)

Attach `EidolonEntityView` to any entity prefab. When other players or NPCs enter your Area of Interest (AoI):
- `EidolonNetworkManager` instantiates the mapped prefab automatically.
- `EidolonEntityView` smooths visual displacement using cubic Hermite interpolation and dead reckoning.
- If a teleport or respawn occurs exceeding `SnapDistanceThreshold`, the view snaps instantly to avoid visual sliding.

---

## 3. Scripting Examples

### Listening to Server Events
```csharp
using UnityEngine;
using Eidolon.Unity;

public class GameUI : MonoBehaviour
{
    private void Start()
    {
        EidolonNetworkManager.Instance.OnConnected.AddListener(OnConnected);
        EidolonNetworkManager.Instance.OnEntitySpawned.AddListener(OnEntitySpawned);
        EidolonNetworkManager.Instance.OnChatMessageReceived.AddListener(OnChatMessage);
    }

    private void OnConnected(ulong accountId)
    {
        Debug.Log($"Connected to Eidolon server with Account: {accountId}");
    }

    private void OnEntitySpawned(uint entityId, byte entityType)
    {
        Debug.Log($"Entity {entityId} (Type {entityType}) spawned into view!");
    }

    private void OnChatMessage(byte channel, string text)
    {
        Debug.Log($"[Chat {channel}] {text}");
    }
}
```

### Casting an Authoritative Ability
```csharp
// Dispatches ability ID 42 targeting entity ID 1005
EidolonNetworkManager.Instance.CastAbility(1005, 42);
```

### Driving High-Speed Vehicles
```csharp
// Transmits land speeder intent: 100% throttle, 0 steering
EidolonNetworkManager.Instance.SendVehicleIntent(
    vehicleType: 1, // LandSpeeder
    throttle: 255,
    steering: 0,
    pitch: 0,
    roll: 0
);
```
