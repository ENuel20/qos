#qos

The agave-scheduler-bindings is a modular extension to attach to the Agave validator client on the Solana blockchain. It allows validators to customize block packing logic in a transparent and secure way, without modifying the main Agave source code
<img width="2988" height="1771" alt="Screenshot 2025-11-21 044233" src="https://github.com/user-attachments/assets/fba068ed-58fb-4f69-ae1f-e24b625a10a3" />


# QOS — Greedy Multi-Client Priority Lane Scheduler

**Programmable Blockspace for Solana**  
Built on Agave’s shared-memory execution pipeline.

---

## Overview

**QOS** is a multi-client, multi-lane external scheduler for Solana. It introduces **programmable blockspace**, allowing wallets, enterprise apps, MEV searchers, and system-level flows to compete fairly inside isolated lanes.

It attaches as a modular extension to the **agave-scheduler-bindings**, without touching validator source code.

QOS proves that Solana can support an Ethereum-style proposal-builder layer — with no performance penalty — because shared memory + zero-copy IPC makes it fast enough to matter.

---

## Architecture at a Glance

QOS is built from four primitives:

1.  **Client-local FIFO queues**
    Each client maintains strict intra-client ordering.

2.  **Global min-max heaps (only queue heads)**
    The scheduler tracks only the *head transaction* of each client — the minimum state required to prioritize globally.

3.  **Shared memory regions (SHAQ) + RTS-Alloc arena**
    All transactions live in a preallocated, zero-copy arena. Written once by TPU → referenced by external scheduler → consumed by BankingStage.

4.  **Unix Domain Socket handshake**
    A lightweight, pointer-only handshake between validator and external scheduler. No copying. No serialization.

**Result:** External scheduling adds **microseconds**, not milliseconds.

---

## Design Goals

### Deterministic Per-Client Ordering
Clients always pop from their own local FIFO queue. No cross-client reordering ensures predictable transaction sequencing for individual senders.

### Greedy Global Prioritization
Across all clients inside a lane, QOS always chooses the **highest fee/CU head**.
If a low-priority client occupies a slot, QOS evicts it via `pop_min` and immediately replaces it with a better head.

### Efficient Multi-Lane Isolation
Each lane maintains two heaps:
*   **Unchecked heap**: For incoming transactions waiting to be verified.
*   **Checked heap**: For verified transactions ready for execution.

Both are min-max heaps holding only queue heads.
**Priority** = `fee_per_CU`.

### Fast & Zero-Copy
QOS uses the same components as Agave:
*   **SHAQ** for shared memory
*   **RTS-Alloc** for allocator-free arenas
*   **Zero-copy transaction passing**
*   **Unix Domain Sockets** for pointer exchange

No bytes move. Only pointers move.

---

## Scheduling Model

### Stage 1 — Client Ingress
*   Client pushes transactions into its **local FIFO queue**.
*   Only the queue head is tracked globally.
*   Ingress order determines *per-client* ordering, not *global* ordering.

### Stage 2 — Unchecked Priority Heap
*   For each client, QOS inserts the head into a **min-max heap**.
*   `pop_max()` yields the global best transaction.
*   After popping, the client’s next head is pushed back (`push_back`).
*   If the heap is at capacity, QOS performs:
    ```rust
    pop_min()  // evict lowest-priority client head
    push(new_client_head)
    ```
    This is the greedy behavior.

### Stage 3 — Checked Heap
*   Same structure, but only validated transactions go here.
*   QOS pulls from this heap when constructing block batches.
*   The greedy nature persists → highest priority always wins.

---

## Lane Isolation

Each lane operates the full pipeline independently:
*   **High-Value Lane**
*   **Normal Lane**
*   **MEV Lane**
*   **TPU Lane** (validator-native)

Per lane features:
*   FIFO queues per client
*   Unchecked min-max heap
*   Checked min-max heap
*   CU budgets & weights
*   Entry constraints (bundles, atomic groups, sequencing)

Weighted lane selection gives each lane deterministic blockspace share.

---

## Compatibility

QOS is fully compatible with the Agave validator:
*   Same shared-memory regions
*   Same RTS-Alloc arena
*   Same zero-copy batching
*   Same handshake protocol
*   No changes to BankingStage or core runtime

It slides between the **network ingress** and the **greedy scheduler**, and consumes the same shared memory transactions.

---

## Why It Works

Solana’s shared-memory pipeline makes proposal-builder systems fast:
*   No mempools
*   No gossip serialization
*   No copying
*   No redundant hashing
*   No syscalls

Other chains pay **milliseconds** for IPC. Solana pays **microseconds**.
This single architectural fact makes QOS **practical, safe, and production-feasible**.

---

## Status

**Research → Prototype → Agave-ready module.**

Current focus:
*   Benchmarks for lane switching
*   Heap saturation stress tests
*   Checked-heap cross-lane fairness validation
*   End-to-end BankingStage integration

---

## Build

To build the project, ensure you have Rust installed and run:

```bash
cargo build --release
```

                                                                                                                                                                                                                                                    
                                                                                                                                                                              
