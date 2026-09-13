# eidolon Wire Protocol & Byte Math Specification

**Status:** Production Engine Complete  
**Classification:** Public Engineering Specification  

---

## 1. The Wire Budget Mandate (<1.2 KB/s)

Streaming raw IEEE 754 32-bit or 64-bit coordinates $(X, Y, Z)$ every tick is mathematically unsustainable at scale:
- 3 floating point numbers $\times$ 4 bytes = 12 bytes.
- Adding heading (4 bytes), velocity (12 bytes), and sequence headers (12 bytes) = 40 bytes per entity update.
- At 20 Hz, an observer seeing 100 entities requires:
  $$\text{Throughput} = 100 \times 40\text{ bytes} \times 20\text{ Hz} = 80,000\text{ bytes/sec} = 80\text{ KB/s}$$
- At $80\text{ KB/s}$ per player, 1,000,000 CCU costs over **$9.6M per month** in cloud network egress.

`eidolon` slashes this footprint by over **98%**, achieving an average wire bandwidth of **460 B/s (0.45 KB/s)** per client.

---

## 2. Asymmetric Spatial Coordinate Quantization

Coordinates are partitioned relative to localized spatial cells ($64\text{m} \times 32\text{m} \times 64\text{m}$):
- **Horizontal $X$ and $Z$ (16 bits each):**
  $$\text{Resolution}_H = \frac{64.0\text{m}}{65,535} \approx 0.0009765\text{m} \quad (0.976\text{ mm})$$
- **Vertical Elevation $Y$ (12 bits):**
  $$\text{Resolution}_V = \frac{32.0\text{m}}{4,095} \approx 0.0078144\text{m} \quad (7.81\text{ mm})$$
- **Discrete Yaw Heading (8 bits):**
  $$\text{Resolution}_{\text{Yaw}} = \frac{360.0^\circ}{256} \approx 1.40625^\circ$$
- **Entity Movement Flags (4 bits):**
  Idle, Walking, Sprinting, Jumping, Falling, Immobilized.

### Exact 7-Byte Wire Bitpacking Layout
Coordinates (44 bits), discrete yaw (8 bits), and movement flags (4 bits) are bitpacked into an exact **7-byte (56-bit)** wire payload:

```text
Byte 0: [ X low 8 bits (0..7) ]
Byte 1: [ X high 8 bits (8..15) ]
Byte 2: [ Z low 8 bits (0..7) ]
Byte 3: [ Z high 8 bits (8..15) ]
Byte 4: [ Y low 8 bits (0..7) ]
Byte 5: [ Y high 4 bits (8..11) | Yaw low 4 bits (0..3) ]
Byte 6: [ Yaw high 4 bits (4..7) | Movement flags (0..3) ]
```

---

## 3. Intent-Based Dead Reckoning Kinematics

Instead of streaming state continuously, the engine transmits intent vectors (velocity, acceleration, heading) only when state diverges beyond configured deadbands:

$$\text{Displacement}(\Delta t) = \mathbf{V}_0 \cdot \Delta t + \frac{1}{2} \mathbf{A}_0 \cdot (\Delta t)^2$$
$$\text{Heading}(\Delta t) = \text{Yaw}_0 + \omega \cdot \Delta\text{ticks}$$

### Predictive Update Triggers
A wire transform packet is dispatched **only** when:
1. **Position Divergence:** $(\mathbf{P}_{\text{actual}} - \mathbf{P}_{\text{predicted}})^2 > 0.0025\text{ m}^2$ (5 centimeters).
2. **Velocity Divergence:** $(\mathbf{V}_{\text{actual}} - \mathbf{V}_{\text{predicted}})^2 > 0.01\text{ (m/s)}^2$ (10 cm/s).
3. **Heading Divergence:** Angular deviation $> 3$ discrete steps ($\approx 4.2^\circ$).
4. **Flag Transition:** Discrete state change (e.g. initiating jump or sprint).
5. **Heartbeat:** Periodic ping every 40 ticks (2.0 seconds) to prevent stale state.

---

## 4. Packet Framing & Channel Protocols

Every datagram begins with a fixed 12-byte header:

```text
 0                   1                   2                   3
 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|          Magic (0x4549)       |    Version    | Channel | Type|
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|        Sequence Number        |       Highest ACK Sequence    |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                 Sliding-Window ACK Bitfield (32 bits)         |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
```

### Channel Types
1. **Unreliable Sequenced (Channel 0):** High-frequency transform deltas. Stale out-of-order packets are dropped instantly; zero ACK overhead.
2. **Ordered Reliable (Channel 1):** Inventory transactions, chat, zone migrations. Uses sliding-window selective ACKs with bounded retransmission queues.
