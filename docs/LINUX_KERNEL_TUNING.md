# Linux Kernel & Low-Level UDP Socket Tuning Guide

This engineering specification establishes the production operating system configuration, socket buffer mathematics, and hardware NIC queue tuning required to host high-concurrency `eidolon` game server pods on Linux (bare-metal, AWS EC2, and Google Cloud GKE).

---

## 1. Core Operating System Network Stack Architecture

In high-concurrency multiplayer engines, Linux network performance is dictated by the interaction between kernel socket ring buffers, hardware NIC receive queues (RSS), softirq interrupt processing, and user-space simulation thread affinity.

```
       Physical Network Interface Card (NIC)
                         │
                         ▼
        Hardware Rx Ring Buffer (ethtool -G)
                         │
                         ▼
        Receive Side Scaling (RSS Multi-Queue)
          ├── IRQ 24 ──> CPU Core 0 (ksoftirqd/0)
          └── IRQ 25 ──> CPU Core 1 (ksoftirqd/1)
                         │
                         ▼
         Kernel Socket Ingress Queue (rmem)
                         │
                         ▼
             recvmmsg / UDP Socket
                         │
                         ▼
        eidolon Server Process (CPU Core 2)
```

---

## 2. Kernel `sysctl.conf` Production Profile

Place the following configuration in `/etc/sysctl.d/99-eidolon.conf` and apply via `sysctl --system`:

```ini
# ====================================================================
# eidolon MMO Production Network Configuration
# ====================================================================

# Maximum socket receive buffer size (16 MB)
net.core.rmem_max = 16777216

# Maximum socket send buffer size (16 MB)
net.core.wmem_max = 16777216

# Default socket receive buffer size (2 MB)
net.core.rmem_default = 2097152

# Default socket send buffer size (2 MB)
net.core.wmem_default = 2097152

# Maximum packets queued on input before processing by kernel softirq
net.core.netdev_max_backlog = 10000

# UDP minimum, pressure, and maximum memory in pages (4KB per page)
# Min: 32MB, Pressure: 64MB, Max: 128MB
net.ipv4.udp_mem = 8192 16384 32768

# Minimum receive buffer size for UDP sockets
net.ipv4.udp_rmem_min = 8192

# Minimum send buffer size for UDP sockets
net.ipv4.udp_wmem_min = 8192

# Disable slow start restart to preserve congestion window across idle ticks
net.ipv4.tcp_slow_start_after_idle = 0

# Enable Path MTU Discovery to prevent silent UDP packet drops
net.ipv4.ip_no_pmtu_disc = 0
```

---

## 3. Network Interface Card (NIC) Tuning

### 3.1 Hardware Ring Buffer Sizing
By default, most cloud Linux distributions configure NIC ring buffers to 256 or 512 descriptors. Under sudden connection bursts or flash mob arrivals, undersized ring buffers cause packet drops at the physical layer before reaching kernel memory.

Query maximum ring sizes:
```bash
ethtool -g eth0
```

Apply maximum hardware descriptor depth (e.g. 4096 descriptors):
```bash
sudo ethtool -G eth0 rx 4096 tx 4096
```

### 3.2 Receive Side Scaling (RSS) & Multi-Queue Hashing
Modern server NICs (such as AWS ENA or Intel 82599) distribute incoming UDP flows across multiple CPU cores via hardware 4-tuple hashing: `(Source IP, Destination IP, Source Port, Destination Port)`.

Assert that UDP 4-tuple hashing is enabled:
```bash
ethtool -k eth0 | grep receive-hashing
ethtool -n eth0 rx-flow-hash udp4
```

If UDP hashing excludes port numbers, enable 4-tuple hashing:
```bash
sudo ethtool -N eth0 rx-flow-hash udp4 sdfn
```

---

## 4. CPU Core Pinning & SoftIRQ Isolation

To eliminate scheduling jitter and CPU cache invalidations, dedicate separate physical cores to Linux interrupt servicing and authoritative simulation ticks.

### 4.1 Isolating the Authoritative Simulation Thread
Pin the 20 Hz simulation tick loop away from Core 0 and Core 1 (which service NIC interrupts):

```bash
# Execute eidolon-server pinned to isolated CPU Core 2
taskset -c 2 ./target/release/eidolon-server
```

In Kubernetes / Agones, specify explicit static CPU manager policies:
```yaml
resources:
  limits:
    cpu: "2"
    memory: "4Gi"
  requests:
    cpu: "2"
    memory: "4Gi"
```

---

## 5. Maximum Transmission Unit (MTU) & Packet Fragmentation

`eidolon` enforces a strict sub-1,200-byte application packet ceiling (`MAX_PACKET_SIZE = 1200`), intentionally sized below standard Ethernet MTU boundaries:

| Layer | Header Size | Cumulative Size |
| :--- | :--- | :--- |
| **Application Payload (L7)** | Up to 1,188 bytes | 1,188 bytes |
| **eidolon Packet Header** | 12 bytes | 1,200 bytes |
| **UDP Header (L4)** | 8 bytes | 1,208 bytes |
| **IPv4 Header (L3)** | 20 bytes | 1,228 bytes |
| **Ethernet II MTU Limit** | **1,500 bytes** | **272 bytes Safety Margin** |

Because total datagram size (1,228 bytes) is strictly less than 1,500 bytes, IP-level packet fragmentation is completely prevented across the public internet.

---

## 6. Agones Kubernetes Host Port Configuration

When deploying on Kubernetes via Agones, always configure `hostPort` or `Passthrough` container port allocation rather than cluster `NodePort` or `ClusterIP` proxies. This eliminates `kube-proxy` iptables / IPVS translation overhead, delivering raw UDP datagrams straight to the server socket.
