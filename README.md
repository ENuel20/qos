#qos

The agave-scheduler-bindings is a modular extension to attach to the Agave validator client on the Solana blockchain. It allows validators to customize block packing logic in a transparent and secure way, without modifying the main Agave source code<img 
 QOS Scheduler – Greedy Multi-Client Priority Lane Scheduler


🔷 QOS — Greedy Multi-Client Priority Lane Scheduler

Programmable Blockspace for Solana
A modular extension built on top of agave-scheduler-bindings, enabling validators to customize block packing logic — without modifying Agave itself.

🚀 Overview

QOS is a multi-client, multi-lane external scheduler designed for Solana’s shared-memory execution pipeline.

It introduces programmable blockspace, allowing:

Wallets

Enterprise apps

MEV searchers

Games

System operations

…to compete fairly inside isolated lanes, each with deterministic ordering and global priority.

Core idea:
Clients get local fairness.
The network gets greedy global prioritization.
Solana keeps microsecond-level speed.

🧱 Architecture at a Glance

QOS stands on four core primitives:

1️⃣ Client-Local FIFO Queues

Each client maintains strict internal ordering.

Only the head of each queue is considered for prioritization.

2️⃣ Global Min-Max Heaps (Heads Only)

A min-max heap holds only queue heads, not full transactions.

Supports pop_max() for best priority and pop_min() for greedy eviction.

3️⃣ Shared Memory Regions (SHAQ) + RTS-Alloc

All transactions live in a zero-copy arena.

Written once → referenced by everyone → never copied.

Designed for the Agave execution pipeline.

4️⃣ Unix Domain Socket Handshake

Scheduler and validator exchange pointers, not data.

Zero serialization. Zero copying. Microsecond overhead.

🎯 Design Goals
✔️ Deterministic Per-Client Ordering

FIFO per client.
No cross-client reordering.
Every client gets guaranteed sequencing.

✔️ Greedy Global Prioritization

Across all clients, QOS always picks the highest fee/CU head.

Low-priority clients are aggressively evicted:

pop_min()   # remove lowest priority client head
push(new_head)


This is pure greedy scheduling.

✔️ Efficient Lane Isolation

Each lane maintains:

client FIFO queues

unchecked min-max heap

checked min-max heap

deterministic pushback

lane CU budgets

lane weights

Lanes operate independently with their own traffic and policies.

⚙️ Three-Stage Scheduling Pipeline
🔹 Stage 1 — Client Ingress

Clients push transactions into their local FIFO queue.

Only the queue head enters global structures.

Arrival order applies only within the client.

🔹 Stage 2 — Unchecked Heap (Pre-Validation)

The head of each client queue goes to a min-max heap.

Scheduler pulls the best via:

pop_max()


After consumption, the client’s next head is inserted:

push_back(next_tx)


When saturated:

pop_min()   # evict lowest-priority head
push(new_head)


Aggressive, greedy, fast.

🔹 Stage 3 — Checked Heap (Post-Validation)

Same mechanism as unchecked.

Contains only validated transactions.

Used to build final batches for BankingStage.

🛣️ Lane Isolation & Blockspace Shaping

Configurable example:

Lane	Purpose	Share
High-Value	enterprise, payments	50%
Normal	wallets, games	40%
MEV	searchers, experiments	10%
TPU Lane	validator-native	separate path

Each lane performs:

Independent priority selection

Deterministic ordering

Atomic group / bundle enforcement

CU budgeting

Weighted contributions to each block

⚡ Why This Works on Solana

Most chains cannot support external builders without huge latency.
Solana can — because of:

🔸 SHAQ Shared Memory

Transaction bytes never move.

🔸 RTS-Alloc Arena

High-performance bump allocator for TPU → scheduler → workers.

🔸 Unix Domain Sockets

Pointer-only handshake.

🔸 Zero-Copy Everywhere

No mempool gossip → no serialization → no hashing → microsecond cost.

This is why Solana can support a proposal-builder layer —
and do it without slowing down.

🔧 Compatibility

QOS integrates directly into Agave’s external scheduling path:

same shared-memory regions

same arena allocator

same handshake protocol

same greedy scheduler interface

same BankingStage expectations

No validator changes required.

📈 Current Status

Prototype stage, built to be:

Clean

Modular

Fast

Agave-compatible

Zero-copy from end to end

Roadmap:

Lane-level benchmarks

Heap saturation tests

Cross-lane fairness validation

End-to-end BankingStage packing                                                                                                                                                                                                                                                      
                                                                                                                                                                                                                   width="3000" height="2000" alt="image" src="https://github.com/user-attachments/assets/6f43dde5-5d98-4800-971b-559e686c1e7b" />
