# FuelVM 作为 Sovereign Rollup 适配 Tondi L1 方案

## 概述

本文档描述如何将 FuelVM 以 **Sovereign Rollup** 形式运行，并使用 Tondi 的 **Ingot 交易**来提交区块数据和状态承诺到 Tondi L1 层。

---

## 现有架构分析：FuelVM DA 层设计

### 当前架构概览

FuelVM 目前的 DA（Data Availability）层设计是**绑定到以太坊（Ethereum）**，通过 `relayer` feature 实现：

```
┌─────────────────────────────────────────────────────────────┐
│                      FuelVM (L2)                            │
│  ┌───────────────────────────────────────────────────────┐  │
│  │  Block Producer                                       │  │
│  │  - 等待 Relayer 同步到指定 DA 高度                      │  │
│  │  - 每个区块关联一个 da_height                          │  │
│  └───────────────────────────────────────────────────────┘  │
│  ┌───────────────────────────────────────────────────────┐  │
│  │  Relayer Service (crates/services/relayer/)           │  │
│  │  - 监听以太坊 FuelMessagePortal 合约事件               │  │
│  │  - 同步 MessageSent / Transaction 事件                │  │
│  │  - 维护 finalized DA 区块高度                          │  │
│  └───────────────────────────────────────────────────────┘  │
│  ┌───────────────────────────────────────────────────────┐  │
│  │  Relayer Database (RocksDB)                           │  │
│  │  - Column::Metadata: 同步元数据                        │  │
│  │  - Column::History: 事件历史（按 DaBlockHeight 索引）   │  │
│  └───────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
                            ↑
                            │ Alloy RPC (JSON-RPC over HTTP/WS)
                            ↓
┌─────────────────────────────────────────────────────────────┐
│              Ethereum (L1 - DA Layer)                       │
│  ┌───────────────────────────────────────────────────────┐  │
│  │  FuelMessagePortal 合约                               │  │
│  │  - MessageSent 事件：跨链消息传递                      │  │
│  │  - Transaction 事件：强制交易（Forced Transactions）   │  │
│  │  - 默认地址: 0x03E4538018285e1c03CCce2F92C9538c87606911│  │
│  └───────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
```

### 核心组件详解

#### 1. Relayer Service

位置：`crates/services/relayer/src/`

```rust
// 配置结构 (config.rs)
pub struct Config {
    /// 合约部署的 DA 区块高度
    pub da_deploy_height: DaBlockHeight,
    /// 以太坊 RPC 端点（支持多个用于法定人数验证）
    pub relayer: Option<Vec<url::Url>>,
    /// 监听的以太坊合约地址
    pub eth_v2_listening_contracts: Vec<Address>,
    /// 日志下载页面大小
    pub log_page_size: u64,
    /// 同步最小间隔（防止 spam DA 节点）
    pub sync_minimum_duration: Duration,
    // ...
}
```

#### 2. 事件监听

```rust
// abi.rs - 监听的以太坊事件
sol! {
    // 跨链消息事件
    event MessageSent(
        bytes32 indexed sender,
        bytes32 indexed recipient,
        uint256 indexed nonce,
        uint64 amount,
        bytes data
    );
    
    // 强制交易事件
    event Transaction(
        uint256 indexed nonce,
        uint64 max_gas,
        bytes canonically_serialized_tx
    );
}
```

#### 3. 同步流程

```rust
// service/run.rs
pub async fn run<R>(relayer: &mut R) -> anyhow::Result<()> {
    // 1. 等待以太坊节点同步完成
    relayer.wait_if_eth_syncing().await?;
    
    // 2. 获取以太坊最新 finalized 区块
    let mut state = state::build_eth(relayer).await?;
    
    // 3. 检查同步差距
    if let Some(eth_sync_gap) = state.needs_to_sync_eth() {
        // 4. 下载事件日志并写入本地数据库
        relayer.download_logs(&eth_sync_gap).await?;
    }
    
    // 5. 更新同步状态
    relayer.update_synced(&state);
}
```

#### 4. 区块生产集成

每个 FuelVM 区块都关联一个 `da_height`：

```rust
// 区块头结构
pub struct ApplicationHeader {
    pub da_height: DaBlockHeight,         // 关联的 DA 区块高度
    pub consensus_parameters_version: ConsensusParametersVersion,
    pub state_transition_bytecode_version: StateTransitionBytecodeVersion,
    // ...
}

// Producer 等待 DA 同步
async fn wait_for_at_least_height(&self, height: &DaBlockHeight) -> anyhow::Result<DaBlockHeight> {
    #[cfg(feature = "relayer")]
    {
        match &self.relayer_synced {
            Some(sync) => {
                sync.await_at_least_synced(height).await?;
                Ok(sync.get_finalized_da_height())
            }
            None => Ok(0u64.into()),
        }
    }
    #[cfg(not(feature = "relayer"))]
    {
        // 没有 relayer 时，da_height 固定为 0
        Ok(0u64.into())
    }
}
```

### 启用方式

```bash
fuel-core run \
    --enable-relayer \
    --relayer=https://eth-mainnet.example.com \
    --relayer-v2-listening-contracts=0x03E4538018285e1c03CCce2F92C9538c87606911 \
    --relayer-da-deploy-height=1000000 \
    --relayer-log-page-size=10000 \
    --relayer-min-duration=5s
```

### 现有设计特点

| 特性 | 说明 |
|------|------|
| **DA 层** | 以太坊（通过合约事件） |
| **数据流向** | L1 → L2（从以太坊拉取事件） |
| **本地存储** | RocksDB 缓存事件历史 |
| **可选性** | `relayer` feature，不启用时 `da_height = 0` |
| **法定人数** | 支持多个 RPC 端点验证 |
| **功能** | 跨链消息 + 强制交易 |

### 现有设计的局限性

1. **单向数据流**：仅支持 L1 → L2，无法将 L2 区块数据提交到 L1
2. **依赖以太坊**：必须运行以太坊节点或使用 RPC 服务
3. **非 Sovereign Rollup**：无法独立验证 L2 状态，依赖以太坊合约

---

## Tondi 替代方案

### 设计目标

将 FuelVM 改造为 **Sovereign Rollup**，使用 Tondi 作为 DA 层：

1. **双向数据流**：L2 区块数据提交到 L1（Tondi）
2. **独立验证**：任何人可以从 Tondi 获取数据验证 L2 状态
3. **去中心化**：使用 Tondi Ingot 机制确保数据可用性

### 架构对比

```
┌─────────────────────────────────────────────────────────────────────────┐
│                           现有架构 vs Tondi 架构                         │
├─────────────────────────────────┬───────────────────────────────────────┤
│         现有架构（以太坊）        │           Tondi 架构                  │
├─────────────────────────────────┼───────────────────────────────────────┤
│                                 │                                       │
│  FuelVM (L2)                    │  FuelVM (Sovereign Rollup)            │
│       ↑                         │       │                               │
│       │ 拉取事件                 │       ↓ 提交区块                       │
│       │                         │                                       │
│  Ethereum (L1)                  │  Tondi (L1)                           │
│  - FuelMessagePortal            │  - Ingot Transaction                  │
│  - MessageSent 事件              │  - Block Batch Payload                │
│  - Transaction 事件              │  - State Commitment                   │
│                                 │                                       │
│  数据流向: L1 → L2               │  数据流向: L2 → L1                     │
│  用途: 跨链消息/强制交易          │  用途: DA + 状态承诺                   │
│                                 │                                       │
└─────────────────────────────────┴───────────────────────────────────────┘
```

### 替代方案：Tondi Ingot Adapter

创建新的服务模块替代 Relayer，实现 L2 → L1 数据提交：

```
┌─────────────────────────────────────────────────────────────┐
│              FuelVM (Sovereign Rollup)                      │
│  ┌───────────────────────────────────────────────────────┐  │
│  │  Block Producer (PoA)                                 │  │
│  │  - 生产区块后通知 Tondi Adapter                        │  │
│  └───────────────────────────────────────────────────────┘  │
│                          │                                  │
│                          ↓ 新区块通知                        │
│  ┌───────────────────────────────────────────────────────┐  │
│  │  Tondi Ingot Adapter (新增)                           │  │
│  │  - 替代 Relayer Service                               │  │
│  │  - 批量聚合区块                                        │  │
│  │  - 构建 Ingot Transaction                             │  │
│  │  - 提交到 Tondi L1                                    │  │
│  └───────────────────────────────────────────────────────┘  │
│  ┌───────────────────────────────────────────────────────┐  │
│  │  Submission Database (RocksDB)                        │  │
│  │  - 待提交区块缓冲                                      │  │
│  │  - 已提交批次记录                                      │  │
│  │  - Tondi 区块高度映射                                  │  │
│  └───────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
                            │
                            ↓ Ingot Transaction
┌─────────────────────────────────────────────────────────────┐
│              Tondi L1 (Data Availability)                   │
│  - Pay2Ingot Output                                         │
│  - FuelVM Block Batch Payload                               │
│  - State Commitment                                         │
│  - 链式结构（parent_ref）                                    │
└─────────────────────────────────────────────────────────────┘
```

### 功能对比

| 功能 | 现有 Relayer | Tondi Adapter |
|------|-------------|---------------|
| **数据方向** | L1 → L2（拉取） | L2 → L1（推送） |
| **DA 层** | 以太坊 | Tondi |
| **提交内容** | 监听事件 | 区块批次 + 状态承诺 |
| **验证方式** | 依赖以太坊合约 | 独立验证 |
| **跨链消息** | ✅ MessageSent | ❌ 不支持（可扩展） |
| **强制交易** | ✅ Transaction | ❌ 不支持（可扩展） |
| **区块数据提交** | ❌ 不支持 | ✅ Ingot Payload |
| **状态承诺** | ❌ 不支持 | ✅ State Root |

### 迁移策略

#### 阶段 1：并行运行（可选）

保留 Relayer 用于跨链消息，同时启用 Tondi Adapter：

```bash
fuel-core run \
    # 保留以太坊 Relayer（跨链消息）
    --enable-relayer \
    --relayer=https://eth-mainnet.example.com \
    # 启用 Tondi Adapter（DA 层）
    --enable-tondi \
    --tondi-rpc-url="http://tondi-node:8545"
```

#### 阶段 2：完全替代

禁用以太坊 Relayer，仅使用 Tondi：

```bash
fuel-core run \
    # 禁用以太坊 Relayer
    # --enable-relayer  (不启用)
    # 启用 Tondi Adapter
    --enable-tondi \
    --tondi-rpc-url="http://tondi-node:8545" \
    --tondi-submission-interval=12s
```

### DA 高度映射

在 Tondi 架构中，重新定义 `da_height` 的含义：

```rust
// 现有设计：da_height = 以太坊区块高度
// Tondi 设计：da_height = Tondi 区块高度（批次确认高度）

pub struct TondiDaHeight {
    /// Tondi L1 区块高度
    pub tondi_block_height: u64,
    /// 批次序号
    pub batch_number: u64,
}

impl From<TondiDaHeight> for DaBlockHeight {
    fn from(h: TondiDaHeight) -> Self {
        // 使用 Tondi 区块高度作为 DA 高度
        DaBlockHeight(h.tondi_block_height)
    }
}
```

---

## 目标架构设计

### 整体架构

```
┌─────────────────────────────────────────────────────────────┐
│              FuelVM (Sovereign Rollup)                      │
│  ┌───────────────────────────────────────────────────────┐  │
│  │  FuelVM Execution Layer                               │  │
│  │  - Transaction Execution (UTXO Model)                 │  │
│  │  - State Management (Sparse Merkle Tree)              │  │
│  │  - Block Production (Instant/Interval/Open modes)     │  │
│  └───────────────────────────────────────────────────────┘  │
│                          │                                  │
│                          ↓ 区块生产完成                      │
│  ┌───────────────────────────────────────────────────────┐  │
│  │  Tondi Ingot Adapter (替代 Ethereum Relayer)          │  │
│  │  - Block Batch Aggregator（批量聚合）                  │  │
│  │  - Commitment Generation（状态承诺）                   │  │
│  │  - Ingot Transaction Builder                          │  │
│  │  - Submission Service（后台提交服务）                  │  │
│  └───────────────────────────────────────────────────────┘  │
│  ┌───────────────────────────────────────────────────────┐  │
│  │  Local Storage (RocksDB)                              │  │
│  │  - Pending blocks buffer                              │  │
│  │  - Submitted batch records                            │  │
│  │  - Tondi height mapping                               │  │
│  └───────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
                        │
                        ↓ (Ingot Transaction - Commit Only)
┌─────────────────────────────────────────────────────────────┐
│              Tondi L1 (Data Availability)                   │
│  ┌───────────────────────────────────────────────────────┐  │
│  │  Ingot Output                                         │  │
│  │  - schema_id: "fuelvm/batch/v1"                       │  │
│  │  - hash_payload: blake3(batch_data)                   │  │
│  │  - lock: PubKey (sequencer authorization)             │  │
│  └───────────────────────────────────────────────────────┘  │
│  ┌───────────────────────────────────────────────────────┐  │
│  │  Ingot Payload                                        │  │
│  │  - Batch Info (序号、高度范围、时间戳)                  │  │
│  │  - Block Data List (交易、收据)                        │  │
│  │  - State Commitment (状态根、收据根)                   │  │
│  │  - Parent Ref (链式结构)                               │  │
│  └───────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
```

### 与现有架构的对比

| 组件 | 现有架构（Ethereum Relayer） | Tondi 架构 |
|------|---------------------------|------------|
| DA 服务 | `crates/services/relayer/` | `crates/services/tondi-ingot-adapter/` |
| 数据库 | Relayer DB (events history) | Submission DB (batch records) |
| 配置 | `RelayerConfig` | `TondiConfig` |
| CLI 参数 | `--enable-relayer` | `--enable-tondi` |
| 数据流向 | L1 → L2 | L2 → L1 |
| DA 高度 | Ethereum block height | Tondi block height |

---

## 核心概念

### 1. Ingot 交易基础

根据 Tondi Ingot 规范，每个 Ingot 交易包含：

- **IngotOutput**: 输出结构，包含：
  - `schema_id`: Schema ID (32B blake3 hash)
  - `hash_payload`: Payload 哈希 (32B blake3 hash)
  - `flags`: 标志位
  - `lock`: 锁定机制（None/PubKey/ScriptHash/MastOnly）
  - `mast_root`: 可选的 MAST 根（用于隐私）

- **IngotWitness**: 见证数据，包含：
  - `payload`: 完整 payload 数据
  - `auth_sigs`: 认证签名
  - `script_reveal`: 脚本揭示（用于 ScriptHash）
  - `mast_proof`: MAST 证明（可选）

### 2. Commit-Only 提交模式

我们采用 **Commit-Only** 模式，将完整的区块数据直接包含在 Ingot 交易中：

- **同步提交**: 区块数据和状态承诺在同一笔交易中提交
- **即时可用**: 数据在 L1 确认后立即可供验证
- **简化流程**: 无需两阶段提交，降低复杂度

## 区块生产与 L1 提交频率对齐

### FuelVM 区块生产模式

FuelVM 支持以下区块生产触发模式（参考 `crates/services/consensus_module/poa/src/config.rs`）：

| 模式 | 说明 | 适用场景 |
|------|------|----------|
| `Instant` | 有交易时立即生产区块 | 测试环境、低延迟需求 |
| `Interval { block_time }` | 固定时间间隔生产区块 | 生产环境，可预测出块 |
| `Open { period }` | 开放期后生产区块 | 收集更多交易 |
| `Never` | 不生产区块 | 被动监听节点 |

### L1 提交频率配置

参考 FuelVM 的共享排序器实现（`crates/services/shared-sequencer`），L1 提交频率需要考虑：

```rust
/// Tondi L1 提交配置
#[derive(Debug, Clone)]
pub struct TondiL1SubmissionConfig {
    /// L1 提交间隔（默认 12 秒，对齐 Tondi 出块时间）
    pub submission_interval: Duration,
    
    /// 最大批次大小（区块数量）
    pub max_batch_size: u32,
    
    /// 最小批次大小（达到后立即提交）
    pub min_batch_size: u32,
    
    /// 强制提交超时（即使未达到 min_batch_size）
    pub force_submission_timeout: Duration,
}

impl Default for TondiL1SubmissionConfig {
    fn default() -> Self {
        Self {
            // 对齐 Tondi L1 出块时间
            submission_interval: Duration::from_secs(12),
            // 推荐批次大小：基于 payload 限制（≤84.5 KiB）
            max_batch_size: 10,
            min_batch_size: 1,
            // 30 秒内必须提交
            force_submission_timeout: Duration::from_secs(30),
        }
    }
}
```

### 频率对齐策略

#### 策略 1: 固定间隔批量提交（上正轨以后才推荐）


**配置示例**:
```bash
# FuelVM 每 2 秒生产一个区块
--poa-interval-period=2s

# Tondi 提交配置：每 12 秒提交，批次包含约 10 个区块
--tondi-submission-interval=12s
--tondi-max-batch-size=10
--tondi-min-batch-size=1
```

#### 策略 2: 动态批量提交

根据区块生产速率和 payload 大小动态调整：

```rust
impl BatchAggregator {
    async fn should_submit(&self, pending_blocks: &[SealedBlock]) -> bool {
        // 条件 1: 达到最大批次大小
        if pending_blocks.len() >= self.config.max_batch_size as usize {
            return true;
        }
        
        // 条件 2: 预估 payload 大小接近限制
        let estimated_size = self.estimate_payload_size(pending_blocks);
        if estimated_size >= MAX_PAYLOAD_SIZE * 80 / 100 {  // 80% 阈值
            return true;
        }
        
        // 条件 3: 超时强制提交
        if self.last_submission.elapsed() >= self.config.force_submission_timeout {
            return pending_blocks.len() >= self.config.min_batch_size as usize;
        }
        
        // 条件 4: 达到提交间隔
        if self.last_submission.elapsed() >= self.config.submission_interval {
            return pending_blocks.len() >= self.config.min_batch_size as usize;
        }
        
        false
    }
}
```

### 批次大小计算

根据 Tondi Ingot payload 限制（推荐 ≤84.5 KiB）：

| 区块交易数 | 预估单区块大小 | 推荐批次大小 |
|-----------|---------------|-------------|
| 0-10 txs  | ~2-5 KB       | 15-20 blocks |
| 10-50 txs | ~5-20 KB      | 4-10 blocks |
| 50+ txs   | ~20-80 KB     | 1-3 blocks |

```rust
/// 预估区块序列化大小
fn estimate_block_size(block: &SealedBlock) -> usize {
    // 区块头固定开销
    const HEADER_SIZE: usize = 256;
    // 每笔交易平均大小
    const AVG_TX_SIZE: usize = 500;
    
    HEADER_SIZE + block.transactions().len() * AVG_TX_SIZE
}

/// 计算批次可容纳的区块数
fn calculate_batch_capacity(blocks: &[SealedBlock]) -> u32 {
    const MAX_PAYLOAD: usize = 84 * 1024;  // 84 KiB
    const BATCH_OVERHEAD: usize = 128;     // TLV 开销
    
    let mut total_size = BATCH_OVERHEAD;
    let mut count = 0u32;
    
    for block in blocks {
        let block_size = estimate_block_size(block);
        if total_size + block_size > MAX_PAYLOAD {
            break;
        }
        total_size += block_size;
        count += 1;
    }
    
    count
}
```

## 实现方案

### 数据结构

```rust
// Fuel Block Schema ID
const FUEL_BLOCK_SCHEMA_ID: &[u8] = b"fuelvm/block/v1";
const FUEL_BATCH_SCHEMA_ID: &[u8] = b"fuelvm/batch/v1";

// 批量区块 Payload 结构（TLV 格式）
struct FuelBlockBatchPayload {
    // TLV_OP_KIND (0x0110)
    op_kind: OpKind,  // 0=Mint (创世批次), 1=Transfer (后续批次)
    
    // TLV_PARENT_REF (0x0111) - 指向父批次的 instance_id
    parent_ref: Option<ParentRef>,
    
    // 自定义 TLV: 批次信息
    batch_info: BatchInfo,
    
    // 自定义 TLV: 区块数据列表
    blocks: Vec<FuelBlockData>,
    
    // 自定义 TLV: 批次状态承诺
    batch_commitment: BatchStateCommitment,
}

struct BatchInfo {
    /// 批次序号（递增）
    batch_number: u64,
    /// 起始区块高度
    start_height: u64,
    /// 结束区块高度
    end_height: u64,
    /// 批次内区块数量
    block_count: u32,
    /// 批次创建时间戳
    timestamp: u64,
}

struct FuelBlockData {
    block_height: u64,
    block_hash: [u8; 32],
    prev_block_hash: [u8; 32],
    transactions_root: [u8; 32],
    transactions: Vec<TransactionData>,
    timestamp: u64,
}

struct BatchStateCommitment {
    /// 批次最终状态根
    final_state_root: [u8; 32],
    /// 批次内所有收据的 Merkle 根
    receipts_root: [u8; 32],
    /// 起始状态根（用于验证）
    initial_state_root: [u8; 32],
}
```

### 核心实现

```rust
use tondi_consensus_core::tx::ingot::*;
use fuel_core_types::blockchain::SealedBlock;
use tokio::time::{Duration, Instant};

pub struct TondiIngotAdapter {
    config: TondiL1SubmissionConfig,
    tondi_rpc: TondiRpcClient,
    schema_id: Hash,
    signer: Signer,
    
    /// 待提交区块缓冲
    pending_blocks: Vec<SealedBlock>,
    /// 上次提交时间
    last_submission: Instant,
    /// 上一批次的 instance_id
    last_batch_instance_id: Option<InstanceId>,
    /// 批次计数器
    batch_counter: u64,
}

impl TondiIngotAdapter {
    /// 添加区块到待提交队列
    pub async fn queue_block(&mut self, block: SealedBlock) -> anyhow::Result<()> {
        self.pending_blocks.push(block);
        
        // 检查是否需要立即提交
        if self.should_submit() {
            self.submit_batch().await?;
        }
        
        Ok(())
    }
    
    /// 检查是否应该提交批次
    fn should_submit(&self) -> bool {
        // 达到最大批次大小
        if self.pending_blocks.len() >= self.config.max_batch_size as usize {
            return true;
        }
        
        // 超时强制提交
        if self.last_submission.elapsed() >= self.config.force_submission_timeout {
            return !self.pending_blocks.is_empty();
        }
        
        // 达到提交间隔且有足够区块
        if self.last_submission.elapsed() >= self.config.submission_interval {
            return self.pending_blocks.len() >= self.config.min_batch_size as usize;
        }
        
        false
    }
    
    /// 提交批次到 Tondi L1
    async fn submit_batch(&mut self) -> anyhow::Result<()> {
        if self.pending_blocks.is_empty() {
            return Ok(());
        }
        
        // 取出待提交区块
        let blocks: Vec<_> = self.pending_blocks.drain(..).collect();
        
        // 1. 构建批次信息
        let batch_info = BatchInfo {
            batch_number: self.batch_counter,
            start_height: blocks.first().unwrap().entity.header().height().into(),
            end_height: blocks.last().unwrap().entity.header().height().into(),
            block_count: blocks.len() as u32,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs(),
        };
        
        // 2. 序列化区块数据
        let block_data_list: Vec<FuelBlockData> = blocks
            .iter()
            .map(|b| self.serialize_block(b))
            .collect::<Result<_, _>>()?;
        
        // 3. 计算批次状态承诺
        let batch_commitment = self.compute_batch_commitment(&blocks)?;
        
        // 4. 构建 payload
        let payload = self.build_payload(
            batch_info,
            block_data_list,
            batch_commitment,
        )?;
        
        // 5. 计算 payload hash
        let payload_hash = blake3::hash(&payload);
        
        // 6. 创建 IngotOutput
        let is_genesis = self.last_batch_instance_id.is_none();
        let ingot_output = IngotOutput::new(
            self.schema_id,
            Hash::from_bytes(*payload_hash.as_bytes()),
            IngotFlags::new(),
            Lock::PubKey(self.signer.public_key()),  // 使用 PubKey 锁定
        );
        
        // 7. 构建并提交交易
        let tx = self.build_ingot_transaction(
            ingot_output,
            payload,
            is_genesis,
        )?;
        
        let signed_tx = self.signer.sign(&tx)?;
        let result = self.tondi_rpc.submit_transaction(signed_tx).await?;
        
        // 8. 更新状态
        self.last_batch_instance_id = Some(result.instance_id);
        self.batch_counter += 1;
        self.last_submission = Instant::now();
        
        tracing::info!(
            batch_number = batch_info.batch_number,
            block_range = %format!("{}-{}", batch_info.start_height, batch_info.end_height),
            "Submitted batch to Tondi L1"
        );
        
        Ok(())
    }
    
    fn build_payload(
        &self,
        batch_info: BatchInfo,
        blocks: Vec<FuelBlockData>,
        commitment: BatchStateCommitment,
    ) -> anyhow::Result<Vec<u8>> {
        use tlv::{TlvWriter, FUEL_BATCH_INFO_TLV, FUEL_BLOCKS_TLV, FUEL_COMMITMENT_TLV};
        
        let mut writer = TlvWriter::new();
        
        // OpKind
        let op_kind = if self.last_batch_instance_id.is_none() {
            OpKind::Mint
        } else {
            OpKind::Transfer
        };
        writer.write_op_kind(op_kind);
        
        // ParentRef（如果有）
        if let Some(parent_id) = &self.last_batch_instance_id {
            writer.write_parent_ref(parent_id);
        }
        
        // 批次信息
        writer.write_custom_tlv(FUEL_BATCH_INFO_TLV, &batch_info)?;
        
        // 区块数据
        writer.write_custom_tlv(FUEL_BLOCKS_TLV, &blocks)?;
        
        // 状态承诺
        writer.write_custom_tlv(FUEL_COMMITMENT_TLV, &commitment)?;
        
        Ok(writer.finish())
    }
    
    fn compute_batch_commitment(
        &self,
        blocks: &[SealedBlock],
    ) -> anyhow::Result<BatchStateCommitment> {
        let first_block = blocks.first().unwrap();
        let last_block = blocks.last().unwrap();
        
        // 收集所有收据并计算 Merkle 根
        let receipts_root = self.compute_receipts_root(blocks)?;
        
        Ok(BatchStateCommitment {
            initial_state_root: *first_block.entity.header().prev_root(),
            final_state_root: *last_block.entity.header().state_root(),
            receipts_root,
        })
    }
}
```

### 后台提交服务

```rust
use fuel_core_services::{Service, ServiceRunner, SharedMutex};
use tokio::sync::mpsc;

pub struct TondiSubmissionService {
    adapter: SharedMutex<TondiIngotAdapter>,
    block_receiver: mpsc::Receiver<SealedBlock>,
    config: TondiL1SubmissionConfig,
}

impl Service for TondiSubmissionService {
    async fn run(&mut self, watcher: &mut StateWatcher) -> anyhow::Result<()> {
        let mut interval = tokio::time::interval(self.config.submission_interval);
        
        loop {
            tokio::select! {
                biased;
                
                // 监听停止信号
                _ = watcher.while_started() => {
                    // 停止前提交剩余区块
                    self.adapter.lock().await.submit_batch().await?;
                    break;
                }
                
                // 接收新区块
                Some(block) = self.block_receiver.recv() => {
                    self.adapter.lock().await.queue_block(block).await?;
                }
                
                // 定时检查提交
                _ = interval.tick() => {
                    let mut adapter = self.adapter.lock().await;
                    if adapter.should_submit() {
                        adapter.submit_batch().await?;
                    }
                }
            }
        }
        
        Ok(())
    }
}
```

## 代码集成点

### 1. 在 Producer 服务中集成

位置：`crates/services/producer/src/block_producer.rs`

```rust
use crate::tondi::TondiBlockNotifier;

impl<ViewProvider, TxPool, Executor, GasPriceProvider, ChainStateProvider>
    Producer<ViewProvider, TxPool, Executor, GasPriceProvider, ChainStateProvider>
{
    /// 区块生产后通知 Tondi 适配器
    pub async fn produce_and_notify(&self, ...) -> anyhow::Result<SealedBlock> {
        let sealed_block = self.produce_and_execute(...).await?;
        
        // 通知 Tondi 提交服务
        if let Some(notifier) = &self.tondi_notifier {
            notifier.notify_block(sealed_block.clone()).await?;
        }
        
        Ok(sealed_block)
    }
}
```

### 2. 在 PoA 服务中集成

位置：`crates/services/consensus_module/poa/src/service.rs`

在区块导入后添加通知：

```rust
async fn produce_block(&mut self, ...) -> anyhow::Result<()> {
    // ... 现有逻辑 ...
    
    // Import the sealed block
    self.block_importer
        .commit_result(Uncommitted::new(
            ImportResult::new_from_local(block.clone(), tx_status, events),
            changes,
        ))
        .await?;
    
    // 通知 Tondi 提交服务
    if let Some(sender) = &self.tondi_block_sender {
        let _ = sender.send(block).await;
    }
    
    Ok(())
}
```

### 3. 配置集成

在 `crates/fuel-core/src/service/config.rs` 中添加配置：

```rust
#[derive(Clone, Debug, Deserialize)]
pub struct TondiConfig {
    /// 是否启用 Tondi L1 提交
    pub enabled: bool,
    
    /// Tondi RPC 地址
    pub rpc_url: String,
    
    /// 提交间隔（默认 12 秒）
    pub submission_interval: Duration,
    
    /// 最大批次大小
    pub max_batch_size: u32,
    
    /// 最小批次大小
    pub min_batch_size: u32,
    
    /// 强制提交超时
    pub force_submission_timeout: Duration,
    
    /// Schema ID（可选，默认使用 "fuelvm/batch/v1"）
    pub schema_id: Option<String>,
}

impl Default for TondiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            rpc_url: "http://localhost:8545".to_string(),
            submission_interval: Duration::from_secs(12),
            max_batch_size: 10,
            min_batch_size: 1,
            force_submission_timeout: Duration::from_secs(30),
            schema_id: None,
        }
    }
}
```

### 4. CLI 参数

在 `bin/fuel-core/src/cli/run.rs` 中添加：

```rust
/// Tondi L1 integration arguments
#[derive(Debug, Clone, clap::Args)]
pub struct TondiArgs {
    /// Enable Tondi L1 block submission
    #[clap(long = "enable-tondi", action)]
    pub enabled: bool,
    
    /// Tondi RPC endpoint
    #[clap(long = "tondi-rpc-url", env, default_value = "http://localhost:8545")]
    pub rpc_url: String,
    
    /// L1 submission interval (aligned with Tondi block time)
    #[clap(long = "tondi-submission-interval", env, default_value = "12s")]
    pub submission_interval: humantime::Duration,
    
    /// Maximum blocks per batch
    #[clap(long = "tondi-max-batch-size", env, default_value = "10")]
    pub max_batch_size: u32,
    
    /// Minimum blocks before submission
    #[clap(long = "tondi-min-batch-size", env, default_value = "1")]
    pub min_batch_size: u32,
    
    /// Force submission timeout
    #[clap(long = "tondi-force-timeout", env, default_value = "30s")]
    pub force_submission_timeout: humantime::Duration,
}
```

## 验证与恢复

### 轻节点验证

轻节点可以通过以下方式验证状态承诺：

1. **获取批次数据**: 从 Tondi L1 获取 Ingot 交易的 payload
2. **验证链式结构**: 检查 `parent_ref` 是否正确指向上一批次
3. **验证状态转换**: 检查 `initial_state_root` == 上一批次的 `final_state_root`
4. **抽样验证**: 对于大批次，可以只验证部分区块

### 完整节点恢复

完整节点可以从 Tondi L1 恢复状态：

```rust
impl TondiIngotAdapter {
    /// 从 L1 恢复区块数据
    pub async fn recover_from_l1(
        &self,
        start_batch: u64,
        end_batch: Option<u64>,
    ) -> anyhow::Result<Vec<SealedBlock>> {
        let mut blocks = Vec::new();
        let mut current_instance = self.find_batch_instance(start_batch).await?;
        
        loop {
            // 获取批次 payload
            let payload = self.tondi_rpc
                .get_ingot_payload(&current_instance)
                .await?;
            
            // 解析区块数据
            let batch = self.parse_batch_payload(&payload)?;
            blocks.extend(batch.blocks);
            
            // 检查是否到达目标批次
            if let Some(end) = end_batch {
                if batch.batch_info.batch_number >= end {
                    break;
                }
            }
            
            // 查找下一批次
            match self.find_next_batch(&current_instance).await? {
                Some(next) => current_instance = next,
                None => break,  // 已是最新批次
            }
        }
        
        Ok(blocks)
    }
}
```

## 性能优化

### 1. 数据压缩

在序列化前压缩区块数据，减少 payload 大小：

```rust
use lz4_flex::compress_prepend_size;

fn compress_payload(data: &[u8]) -> Vec<u8> {
    compress_prepend_size(data)
}
```

### 2. 增量状态更新

只提交状态差异，而不是完整状态（可选优化）。

### 3. 并行序列化

对于大批次，并行序列化各区块：

```rust
use rayon::prelude::*;

let block_data_list: Vec<FuelBlockData> = blocks
    .par_iter()
    .map(|b| serialize_block(b))
    .collect::<Result<_, _>>()?;
```

---

## Tondi Reorg 处理与同步机制

### 问题分析

在 Sovereign Rollup 架构中，FuelVM 将区块批次提交到 Tondi L1 作为数据可用性层。当 Tondi 发生 reorg 时，需要考虑以下场景：

```
时间线示例:
  T0: FuelVM 生产区块 [F1, F2, F3]
  T1: 提交批次 B1 到 Tondi (包含 F1, F2, F3)
  T2: Tondi 区块 T100 包含了 B1 (1 确认)
  T3: Tondi 发生 reorg，T100 被移除
  T4: 批次 B1 状态未知 - 可能丢失或在新链的不同位置
```

### Tondi 的 Finality 特性

根据 Tondi GHOSTDAG 共识机制：

| 特性 | 值 | 说明 |
|-----|-----|------|
| 确认时间 | 1-2 秒 | 快速软确认 |
| 最终性确认 | 10 个确认 | DAA score 排序后达到单一活跃状态 |
| 通知机制 | `VirtualChainChanged` | 包含 `added` 和 `removed` 区块哈希 |
| Reorg 深度 | 通常 ≤ 3 块 | GHOSTDAG 的 k-cluster 特性限制 reorg 深度 |

### 批次状态机

```rust
/// 批次在 Tondi 上的状态
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BatchL1Status {
    /// 已提交但未确认（在 mempool 或 0 确认）
    Pending,
    /// 已包含在 Tondi 区块中，但未达到 finality
    Included { 
        tondi_block: Hash,
        confirmations: u8,
    },
    /// 达到 finality（≥6 确认）
    Finalized {
        tondi_block: Hash,
        daa_score: u64,
    },
    /// 被 reorg 移除，需要重新提交
    Orphaned,
    /// 重新提交中
    Resubmitting,
}
```

### 同步策略设计

#### 1. 订阅 Tondi 通知

```rust
use tondi_consensus_notify::notification::{
    Notification, VirtualChainChangedNotification
};

pub struct TondiSyncService {
    /// 批次状态追踪
    batch_tracker: BTreeMap<u64, BatchRecord>,
    /// Tondi 通知订阅
    notification_rx: Receiver<Notification>,
    /// 待确认批次（batch_number -> tondi_block_hash）
    pending_confirmations: HashMap<u64, Hash>,
}

impl TondiSyncService {
    /// 处理 Tondi 链变化通知
    async fn handle_virtual_chain_changed(
        &mut self,
        notification: VirtualChainChangedNotification,
    ) -> anyhow::Result<()> {
        // 1. 处理被移除的区块（reorg）
        for removed_hash in notification.removed_chain_block_hashes.iter() {
            self.handle_removed_block(*removed_hash).await?;
        }
        
        // 2. 处理新增的区块（确认）
        for (added_hash, acceptance_data) in notification
            .added_chain_block_hashes
            .iter()
            .zip(notification.added_chain_blocks_acceptance_data.iter())
        {
            self.handle_added_block(*added_hash, acceptance_data).await?;
        }
        
        // 3. 更新确认数
        self.update_confirmations().await?;
        
        Ok(())
    }
    
    /// 处理被 reorg 移除的区块
    async fn handle_removed_block(&mut self, removed_hash: Hash) -> anyhow::Result<()> {
        // 检查是否有批次在该区块中
        let affected_batches: Vec<_> = self.pending_confirmations
            .iter()
            .filter(|(_, block_hash)| **block_hash == removed_hash)
            .map(|(batch_num, _)| *batch_num)
            .collect();
        
        for batch_num in affected_batches {
            tracing::warn!(
                batch = batch_num,
                block = %removed_hash,
                "Batch orphaned due to Tondi reorg"
            );
            
            // 标记批次为 Orphaned
            if let Some(record) = self.batch_tracker.get_mut(&batch_num) {
                record.l1_status = BatchL1Status::Orphaned;
            }
            
            // 从待确认列表移除
            self.pending_confirmations.remove(&batch_num);
            
            // 触发重新提交
            self.schedule_resubmission(batch_num).await?;
        }
        
        Ok(())
    }
}
```

#### 2. Finality 确认策略

```rust
/// Finality 配置
pub struct FinalityConfig {
    /// 需要的最小确认数（默认 6）
    pub required_confirmations: u8,
    /// 确认检查间隔
    pub check_interval: Duration,
    /// 最大等待时间（超时后警告但不阻塞）
    pub max_wait_time: Duration,
}

impl TondiSyncService {
    /// 检查批次是否达到 finality
    async fn check_batch_finality(&self, batch_num: u64) -> anyhow::Result<bool> {
        let record = self.batch_tracker.get(&batch_num)
            .ok_or_else(|| anyhow::anyhow!("Batch {} not found", batch_num))?;
        
        match &record.l1_status {
            BatchL1Status::Included { confirmations, .. } => {
                Ok(*confirmations >= self.finality_config.required_confirmations)
            }
            BatchL1Status::Finalized { .. } => Ok(true),
            _ => Ok(false),
        }
    }
    
    /// 更新所有待确认批次的确认数
    async fn update_confirmations(&mut self) -> anyhow::Result<()> {
        let current_daa_score = self.get_current_daa_score().await?;
        
        for (batch_num, block_hash) in self.pending_confirmations.iter() {
            let block_daa_score = self.get_block_daa_score(*block_hash).await?;
            let confirmations = current_daa_score.saturating_sub(block_daa_score);
            
            if let Some(record) = self.batch_tracker.get_mut(batch_num) {
                if confirmations >= self.finality_config.required_confirmations as u64 {
                    record.l1_status = BatchL1Status::Finalized {
                        tondi_block: *block_hash,
                        daa_score: block_daa_score,
                    };
                    tracing::info!(
                        batch = batch_num,
                        confirmations = confirmations,
                        "Batch reached finality"
                    );
                } else {
                    record.l1_status = BatchL1Status::Included {
                        tondi_block: *block_hash,
                        confirmations: confirmations as u8,
                    };
                }
            }
        }
        
        Ok(())
    }
}
```

#### 3. 重新提交机制

```rust
impl TondiIngotAdapter {
    /// 重新提交被 orphaned 的批次
    async fn resubmit_batch(&mut self, batch_num: u64) -> anyhow::Result<()> {
        // 1. 从本地存储获取批次数据
        let batch_record = self.storage.get_batch_record(batch_num)?
            .ok_or_else(|| anyhow::anyhow!("Batch {} not found in storage", batch_num))?;
        
        // 2. 检查是否已经在新链中（可能已被包含在其他区块）
        if self.check_batch_in_chain(&batch_record).await? {
            tracing::info!(batch = batch_num, "Batch already in chain, skipping resubmission");
            return Ok(());
        }
        
        // 3. 重新构建并提交 Ingot 交易
        tracing::info!(batch = batch_num, "Resubmitting orphaned batch");
        
        let ingot_tx = self.build_ingot_transaction(&batch_record)?;
        let txid = self.tondi_rpc.submit_transaction(ingot_tx).await?;
        
        // 4. 更新批次状态
        self.batch_tracker.get_mut(&batch_num)
            .map(|r| r.l1_status = BatchL1Status::Resubmitting);
        
        Ok(())
    }
    
    /// 检查批次是否已在链中（通过 schema_id 和 batch_number）
    async fn check_batch_in_chain(&self, batch_record: &BatchRecord) -> anyhow::Result<bool> {
        // 查询 Tondi 的 Ingot UTXO 索引
        let existing = self.tondi_rpc
            .get_ingot_by_schema_and_batch(
                &self.schema_id,
                batch_record.batch_info.batch_number,
            )
            .await?;
        
        Ok(existing.is_some())
    }
}
```

### FuelVM 区块 Finality 语义

在 Sovereign Rollup 模式下，FuelVM 区块的 finality 有两层含义：

```
┌─────────────────────────────────────────────────────────────┐
│ FuelVM Block Finality Levels                                │
├─────────────────────────────────────────────────────────────┤
│ Level 1: L2 Finality (PoA 签名)                             │
│   - 区块由授权 sequencer 签名                                │
│   - 立即可用于本地查询                                       │
│   - 可能被 L1 reorg 影响                                    │
├─────────────────────────────────────────────────────────────┤
│ Level 2: L1 Soft Confirmation (1-5 确认)                    │
│   - 批次已包含在 Tondi 区块中                                │
│   - 大多数情况下不会被 reorg                                 │
│   - 可用于大多数应用场景                                     │
├─────────────────────────────────────────────────────────────┤
│ Level 3: L1 Finality (≥6 确认)                              │
│   - 批次达到 Tondi finality                                  │
│   - 不可逆转                                                 │
│   - 适用于高价值交易和跨链桥接                                │
└─────────────────────────────────────────────────────────────┘
```

### 与 FuelVM Relayer 的关键差异

| 维度 | Ethereum Relayer | Tondi Adapter |
|-----|------------------|---------------|
| 数据流向 | L1 → L2 (拉取事件) | L2 → L1 (推送批次) |
| Finality 源 | Ethereum `finalized` 标签 | Tondi DAA score + 6 确认 |
| Reorg 处理 | 只读取 finalized 区块 | 需要主动处理 reorg 和重新提交 |
| 状态管理 | 简单高度追踪 | 批次状态机 + 确认追踪 |
| DA 高度含义 | 最后同步的 Eth 高度 | 最后确认的 Tondi 高度 |

### 错误恢复流程

```
┌──────────────┐    提交成功     ┌──────────────┐
│   Pending    │ ────────────→  │   Included   │
└──────────────┘                 └──────────────┘
       │                              │  │
       │ 提交失败                      │  │ reorg
       ↓                              │  ↓
┌──────────────┐    重新提交    ┌──────────────┐
│    Failed    │ ←────────────  │   Orphaned   │
└──────────────┘                └──────────────┘
       │                              │
       │ 重试成功                      │ 重新提交
       ↓                              ↓
┌──────────────┐    ≥6 确认    ┌──────────────┐
│   Included   │ ────────────→  │  Finalized   │
└──────────────┘                └──────────────┘
```

### 配置示例

```toml
# fuel-core 配置文件
[tondi]
enabled = true
rpc_url = "http://localhost:16210"

# Finality 配置
required_confirmations = 6
finality_check_interval = "2s"
max_finality_wait = "60s"

# Reorg 处理
resubmit_delay = "5s"
max_resubmit_attempts = 3
orphan_detection_enabled = true
```

## 安全考虑

### 1. 签名验证

使用 `Lock::PubKey` 锁定 Ingot 输出，确保只有授权节点可以提交区块。

### 2. 批次顺序验证

使用 `parent_ref` 确保批次顺序，防止重放或乱序攻击。

### 3. 状态根连续性

验证每个批次的 `initial_state_root` 与上一批次的 `final_state_root` 一致。

### 4. 数据可用性

确保 payload 数据在 L1 上可访问，支持数据可用性验证。

## 部署步骤

### 1. 添加依赖

在 `Cargo.toml` 中添加（Tondi 在父目录）：

```toml
[dependencies]
tondi-consensus-core = { path = "../../Tondi/consensus/core" }
blake3 = "1.5"
lz4_flex = "0.11"
```

### 2. 创建适配器模块

创建 `crates/services/tondi-ingot-adapter/` 目录和相关文件。

### 3. 集成到 PoA 服务

修改 `MainTask` 以支持 Tondi L1 提交通知。

### 4. 配置

在配置文件或命令行中添加 Tondi 相关配置：

```bash
fuel-core run \
    --poa-interval-period=2s \
    --enable-tondi \
    --tondi-rpc-url="http://tondi-node:8545" \
    --tondi-submission-interval=12s \
    --tondi-max-batch-size=10
```

## 代码修改清单

### 需要修改的文件

| 文件 | 修改内容 |
|------|----------|
| `crates/fuel-core/src/service/config.rs` | 添加 `TondiConfig` 配置结构 |
| `crates/fuel-core/src/service/sub_services.rs` | 初始化 Tondi Adapter 服务 |
| `crates/services/consensus_module/poa/src/service.rs` | 区块生产后发送通知 |
| `bin/fuel-core/src/cli/run.rs` | 添加 `TondiArgs` CLI 参数 |
| `Cargo.toml` (workspace) | 添加 `tondi-ingot-adapter` 成员 |

### 需要新建的文件

```
crates/services/tondi-ingot-adapter/
├── Cargo.toml
├── src/
│   ├── lib.rs              # 模块入口
│   ├── config.rs           # 配置结构
│   ├── adapter.rs          # 核心适配器
│   ├── service.rs          # 后台提交服务
│   ├── payload.rs          # Payload 构建
│   ├── storage.rs          # 本地存储
│   └── ports.rs            # 接口定义
└── tests/
    └── integration.rs      # 集成测试
```

### Feature Flag 设计

```toml
# crates/fuel-core/Cargo.toml
[features]
default = ["rocksdb"]
relayer = ["fuel-core-relayer"]           # 现有以太坊 Relayer
tondi = ["fuel-core-tondi-ingot-adapter"] # 新增 Tondi Adapter

# 可以同时启用两者（过渡阶段）
# 或仅启用 tondi（完全替代）
```

---

## 参考资源

### Tondi 相关
- [Tondi Ingot 规范](../../Tondi/docs/ingot/INGOT_SPECS.md)
- [Tondi 共识核心](../../Tondi/consensus/core/)

### FuelVM 相关
- [现有 Relayer 实现](../crates/services/relayer/src/service.rs)
- [Relayer 配置](../crates/services/relayer/src/config.rs)
- [PoA 配置](../crates/services/consensus_module/poa/src/config.rs)
- [共享排序器实现](../crates/services/shared-sequencer/src/lib.rs)
- [区块生产服务](../crates/services/producer/src/block_producer.rs)

---

## 实施计划

### 阶段 1：基础框架（1-2 周）

1. ☑ 创建 `tondi-ingot-adapter` crate 骨架
2. ☑ 定义配置结构和 CLI 参数
3. ☑ 实现基本的 Ingot Transaction 构建
4. ☑ 添加 `tondi` feature flag

### 阶段 2：核心功能（2-3 周）

1. ☑ 实现批量聚合逻辑
2. ☑ 实现后台提交服务
3. ☑ 集成到 PoA 服务（CLI 和配置）
4. ☑ 实现本地存储（待提交缓冲、批次记录）

### 阶段 3：测试与优化（1-2 周）

1. ☐ 编写单元测试
2. ☐ 编写集成测试（模拟 Tondi 节点）
3. ☐ 性能测试和优化
4. ☐ 压缩优化（lz4）

### 阶段 4：生产就绪（1 周）

1. ☐ 错误处理和重试机制
2. ☐ 监控指标（Prometheus）
3. ☐ 文档完善
4. ☐ 端到端测试
