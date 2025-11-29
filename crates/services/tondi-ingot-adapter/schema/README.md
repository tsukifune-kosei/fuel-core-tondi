# FuelVM Ingot Schema

This directory contains the Ingot schema definition for FuelVM block batches on Tondi L1.

## Schema ID

```
schema_string: "fuelvm/batch/v1"
schema_id:     blake3("fuelvm/batch/v1")
```

The schema_id is computed at runtime by `Config::schema_id_bytes()`.

## Files

- `fuelvm_batch.json` - Complete schema definition including:
  - Payload format (BatchHeader + TLV)
  - Authentication (Schnorr signatures via IngotOutput.lock)
  - Validation rules
  - Truncation strategy
  - L2 indexing requirements

## Payload Structure

```
┌─────────────────────────────────────────────────────────────┐
│  BatchHeader (45 bytes, fixed)                               │
│  - version: u8 = 1                                           │
│  - start_height: u64                                         │
│  - block_count: u32                                          │
│  - parent_hash: [u8; 32]  (L2 chain continuity)              │
├─────────────────────────────────────────────────────────────┤
│  TLV Fields                                                  │
│  - 0x1001 FUEL_BLOCKS: Postcard-serialized block data        │
│  - 0x1002 FUEL_COMMITMENT: State roots for ZK/bridges        │
└─────────────────────────────────────────────────────────────┘
```

## Ingot Integration

```
IngotOutput:
  - schema_id: blake3("fuelvm/batch/v1")
  - hash_payload: blake3(full_payload)
  - lock: PubKey { sequencer_pubkey, CopperootSchnorr }

IngotWitness:
  - payload: FuelBatchPayload
  - auth_sigs: [64-byte raw Schnorr signature]
```

## Security

| Mechanism | Description |
|-----------|-------------|
| Sequencer Auth | `Lock::PubKey` with `compute_ingot_sig_msg()` |
| Chain Continuity | `parent_hash` in BatchHeader |
| Data Binding | `hash_payload` = blake3(payload) |
| Truncation | Invalid block → accept valid prefix only |

## Usage

```rust
use fuel_core_tondi_ingot_adapter::Config;

// Get schema ID bytes
let config = Config::default();
let schema_id = config.schema_id_bytes();
// Result: blake3("fuelvm/batch/v1")
```

## See Also

- `/docs/L1-L2 Reorg-Proof Sync Paradigms A & B.md` - Complete data flow
- `../src/payload.rs` - Payload encoding/decoding implementation
- `../src/adapter.rs` - Transaction building with Ingot sighash

