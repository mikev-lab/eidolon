# Eidolon Godot 4 Client Addon: Turnkey MMO Integration

The Eidolon Godot 4 Client Addon provides seamless MMO client networking for Godot 4.0+ (.NET / C#), exploiting direct 1:1 coordinate parity (right-handed, Y-up meters) with zero coordinate conversions, Godot Autoload singletons, signal-based event pipelines, and smooth `Node3D` entity interpolation.

---

## 1. Quickstart in 3 Minutes

### Step 1: Install the Addon
Copy the `bindings/godot/` folder into your Godot project's `addons/eidolon/` folder:

```text
MyGodotGame/
├── addons/
│   └── eidolon/
│       ├── plugin.cfg
│       ├── EidolonNetwork.cs
│       ├── EidolonEntityNode3D.cs
│       └── EidolonGodotConfig.cs
```

Place the compiled native binary (`libeidolon.dylib` on macOS, `eidolon.dll` on Windows, or `libeidolon.so` on Linux) into your project root or `addons/eidolon/native/`.

### Step 2: Register Autoload Singleton
1. In Godot, go to **Project -> Project Settings -> Autoload**.
2. Set **Path** to `res://addons/eidolon/EidolonNetwork.cs` and **Node Name** to `EidolonNetwork`.
3. Click **Add**.

### Step 3: Create a Configuration Resource
1. In the FileSystem dock, right-click and choose **Create New -> Resource...**.
2. Search for `EidolonGodotConfig` and save it as `res://default_eidolon_config.tres`.
3. In the Inspector, configure:
   - **Server Host:** `127.0.0.1` (or your server IP)
   - **Server Port:** `7777`
   - **Density Profile:** 1 (Standard MMO)

Select `EidolonNetwork` in Project Settings (or your scene) and assign `default_eidolon_config.tres` to the **Config** property.

---

## 2. Scripting Examples

### Listening to Server Signals
```csharp
using Godot;
using Eidolon.GodotEngine;

public partial class GameHUD : Control
{
    public override void _Ready()
    {
        EidolonNetwork.Instance.Connected += OnConnected;
        EidolonNetwork.Instance.EntitySpawned += OnEntitySpawned;
        EidolonNetwork.Instance.ChatMessageReceived += OnChatMessage;
    }

    private void OnConnected(ulong accountId)
    {
        GD.Print($"Connected to Eidolon server with Account: {accountId}");
    }

    private void OnEntitySpawned(uint entityId, uint entityType, Vector3 position)
    {
        GD.Print($"Entity {entityId} (Type {entityType}) spawned at {position}!");
    }

    private void OnChatMessage(byte channel, uint senderId, string message)
    {
        GD.Print($"[Chat {channel}] Entity {senderId}: {message}");
    }
}
```

### Transmitting Local Player Intent at 20 Hz
```csharp
using Godot;
using Eidolon.GodotEngine;

public partial class PlayerController : CharacterBody3D
{
    private double _timeSinceLastIntent = 0.0;
    private const double IntentInterval = 1.0 / 20.0; // 20 Hz / 50ms

    public override void _PhysicsProcess(double delta)
    {
        _timeSinceLastIntent += delta;
        if (_timeSinceLastIntent < IntentInterval)
        {
            return;
        }
        _timeSinceLastIntent = 0.0;

        Vector2 inputDir = Input.GetVector("ui_left", "ui_right", "ui_up", "ui_down");
        Vector3 direction = (Transform.Basis * new Vector3(inputDir.X, 0, inputDir.Y)).Normalized();
        Vector3 velocity = direction * 6.0f;

        float yawDeg = RotationDegrees.Y;
        byte flags = 0;
        if (Input.IsActionPressed("ui_accept"))
        {
            flags |= 0x01; // Jump
        }

        EidolonNetwork.Instance.SendIntent(velocity.X, velocity.Z, yawDeg, flags);
    }
}
```
