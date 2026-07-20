#![cfg_attr(feature = "tco", allow(incomplete_features))]
#![cfg_attr(feature = "tco", feature(explicit_tail_calls))]
use alloy_primitives::hex::ToHexExt;
use alloy_provider::RootProvider;
use alloy_rpc_client::RpcClient;
use alloy_transport::layers::RetryBackoffLayer;
use clap::Parser;
#[cfg(feature = "openvm-backend")]
use openvm_benchmarks_prove::util::BenchmarkCli;
#[cfg(feature = "openvm-backend")]
use openvm_circuit::{arch::*, openvm_stark_sdk::openvm_stark_backend::p3_field::PrimeField32};
use openvm_client_executor::{
    io::{
        AccountInput, AncestorHeadersInput, BytecodeInput, BytecodesInput, ClientExecutorInput,
        ClientExecutorInputWithState, ClientWitnessInput, CurrentBlockInput, StateTrieHeader,
        StateTrieInput, StorageTrieHeader, StorageTrieInput, WitnessAccess,
    },
    ChainVariant, ClientExecutor, CHAIN_ID_ETH_MAINNET,
};
use openvm_host_executor::HostExecutor;
#[cfg(feature = "openvm-backend")]
pub use openvm_native_circuit::NativeConfig;

#[cfg(feature = "openvm-backend")]
use openvm_sdk::{
    config::{SdkVmBuilder, SdkVmConfig},
    keygen::{AggProvingKey, AppProvingKey},
    prover::verify_app_proof,
    types::VersionedVmStarkProof,
    DefaultStarkEngine, Sdk, StdIn,
};
#[cfg(feature = "openvm-backend")]
use openvm_stark_sdk::{
    config::baby_bear_poseidon2::BabyBearPoseidon2Config, engine::StarkFriEngine,
};
#[cfg(feature = "openvm-backend")]
use openvm_transpiler::{elf::Elf, openvm_platform::memory::MEM_SIZE};
pub use reth_primitives;
use reth_trie::TrieAccount;
use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    sync::LazyLock,
};
use tracing::{info, info_span};

use cargo_metadata::MetadataCommand;
use ceno_cli::sdk as ceno_sdk;
use ceno_emul::{Platform, Program};
use ceno_host::{CenoStdin, Item, WORD_ALIGNMENT};
use ceno_zkvm::e2e::{
    run_e2e_full_trace_verify, run_e2e_single_shard_debug_verify, setup_platform, MultiProver,
    Preset,
};
use gkr_iop::cpu::default_backend_config;

struct SpanTiming {
    name: &'static str,
    start: std::time::Instant,
}

struct SpanMetricsLayer;

impl<S> tracing_subscriber::Layer<S> for SpanMetricsLayer
where
    S: tracing::Subscriber + for<'lookup> tracing_subscriber::registry::LookupSpan<'lookup>,
{
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        id: &tracing::Id,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if let Some(span) = ctx.span(id) {
            span.extensions_mut().insert(SpanTiming {
                name: attrs.metadata().name(),
                start: std::time::Instant::now(),
            });
        }
    }

    fn on_close(&self, id: tracing::Id, ctx: tracing_subscriber::layer::Context<'_, S>) {
        let Some(span) = ctx.span(&id) else {
            return;
        };
        let extensions = span.extensions();
        let Some(timing) = extensions.get::<SpanTiming>() else {
            return;
        };
        let metric = format!("{}_time_ms", timing.name);
        metrics::gauge!(metric).set(timing.start.elapsed().as_secs_f64() * 1000.0);
    }
}

fn snapshot_to_json(snapshot: metrics_util::debugging::Snapshot) -> serde_json::Value {
    use metrics_util::{debugging::DebugValue, MetricKind};
    use serde_json::json;

    let mut gauges = Vec::new();
    let mut counters = Vec::new();
    let mut histograms = Vec::new();

    for (key, _, _, value) in snapshot.into_vec() {
        let labels =
            key.key().labels().map(|label| json!([label.key(), label.value()])).collect::<Vec<_>>();
        let entry = match value {
            DebugValue::Counter(value) => json!({
                "labels": labels,
                "metric": key.key().name(),
                "value": value.to_string(),
            }),
            DebugValue::Gauge(value) => json!({
                "labels": labels,
                "metric": key.key().name(),
                "value": value.into_inner().round().to_string(),
            }),
            DebugValue::Histogram(values) => json!({
                "labels": labels,
                "metric": key.key().name(),
                "value": values
                    .into_iter()
                    .map(|value| value.into_inner().to_string())
                    .collect::<Vec<_>>(),
            }),
        };
        match key.kind() {
            MetricKind::Counter => counters.push(entry),
            MetricKind::Gauge => gauges.push(entry),
            MetricKind::Histogram => histograms.push(entry),
        }
    }

    json!({
        "gauge": gauges,
        "counter": counters,
        "histogram": histograms,
    })
}

fn run_with_metric_collection<F>(output_env: &str, f: F) -> eyre::Result<()>
where
    F: FnOnce() -> eyre::Result<()>,
{
    let output_path = std::env::var_os(output_env).map(PathBuf::from);
    let recorder = metrics_util::debugging::DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    let _ = recorder.install();

    let result = f();

    if let Some(output_path) = output_path {
        if let Some(parent) = output_path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        let value = snapshot_to_json(snapshotter.snapshot());
        fs::write(output_path, serde_json::to_vec_pretty(&value)?)?;
    }

    result
}

fn write_raw_hint_bytes(hints: &mut CenoStdin, bytes: &[u8]) {
    let end_of_data = bytes.len();
    let mut data = bytes.to_vec();
    data.resize(data.len().next_multiple_of(WORD_ALIGNMENT), 0);
    hints.items.push(Item { data, end_of_data });
}

fn write_ceno_client_input(
    hints: &mut CenoStdin,
    client_input: &ClientExecutorInput,
) -> eyre::Result<()> {
    let (
        current_block_input,
        ancestor_headers_input,
        state_trie_input,
        storage_trie_inputs,
        bytecodes_input,
    ): (
        CurrentBlockInput,
        AncestorHeadersInput,
        StateTrieInput,
        Vec<StorageTrieInput>,
        BytecodesInput,
    ) = client_input.clone().into();

    let storage_trie_by_hash = storage_trie_inputs
        .into_iter()
        .map(|storage_trie| (storage_trie.hashed_address, storage_trie))
        .collect::<BTreeMap<_, _>>();
    let bytecode_by_hash = bytecodes_input
        .bytecodes
        .into_iter()
        .map(|bytecode| (bytecode.hash_slow(), bytecode))
        .collect::<BTreeMap<_, _>>();
    let input_with_state = ClientExecutorInputWithState::build(client_input.clone())?;
    let (_, witness_order) = ClientExecutor
        .execute_recording_witness_order(ChainVariant::Mainnet, client_input.clone())?;
    let mut post_update_witness_count = 0;
    let mut after_state_trie = false;
    for access in &witness_order {
        match access {
            WitnessAccess::StateTrie => after_state_trie = true,
            WitnessAccess::Account(_) | WitnessAccess::StorageTrie(_) if after_state_trie => {
                post_update_witness_count += 1;
            }
            _ => {}
        }
    }
    let account_by_hash = witness_order
        .iter()
        .filter_map(|a| match a {
            WitnessAccess::Account(h) => Some(*h),
            _ => None,
        })
        .map(|hash| {
            input_with_state
                .state
                .state_trie
                .get_rlp::<TrieAccount>(hash.as_slice())
                .map(|account| (hash, account))
                .map_err(Into::into)
        })
        .collect::<eyre::Result<BTreeMap<_, _>>>()?;

    hints.write(&ancestor_headers_input)?;
    hints.write(&current_block_input)?;
    hints.write(&StateTrieHeader {
        num_nodes: state_trie_input.num_nodes,
        post_update_witness_count,
    })?;

    for access in witness_order {
        match access {
            WitnessAccess::Account(hash) => {
                let account = account_by_hash
                    .get(&hash)
                    .ok_or_else(|| eyre::eyre!("missing account for recorded lookup hash {hash}"))?
                    .clone();
                hints.write(&ClientWitnessInput::Account(AccountInput {
                    hashed_address: hash,
                    account,
                }))?;
            }
            WitnessAccess::StorageTrie(hash) => {
                let storage_trie = storage_trie_by_hash
                    .get(&hash)
                    .ok_or_else(|| {
                        eyre::eyre!("missing storage trie for recorded lookup hash {hash}")
                    })?
                    .clone();
                hints.write(&ClientWitnessInput::StorageTrie(StorageTrieHeader {
                    hashed_address: storage_trie.hashed_address,
                    num_nodes: storage_trie.num_nodes,
                }))?;
                write_raw_hint_bytes(hints, storage_trie.bytes.as_ref());
            }
            WitnessAccess::Bytecode(hash) => {
                let bytecode = bytecode_by_hash
                    .get(&hash)
                    .ok_or_else(|| eyre::eyre!("missing bytecode for recorded lookup hash {hash}"))?
                    .clone();
                hints.write(&ClientWitnessInput::Bytecode(BytecodeInput { bytecode }))?;
            }
            WitnessAccess::StateTrie => {
                write_raw_hint_bytes(hints, state_trie_input.bytes.as_ref());
            }
        }
    }

    Ok(())
}
#[cfg(feature = "openvm-backend")]
use serde_json::json;

mod cli;
use cli::ProviderArgs;

/// Enum representing the execution mode of the host executable.
#[derive(Debug, Clone, clap::ValueEnum)]
pub enum BenchMode {
    /// Execute natively on host.
    ExecuteHost,
    /// Execute the VM without generating a proof.
    Execute,
    /// Execute the VM with metering to get segments information.
    ExecuteMetered,
    /// Generate sequence of app proofs for continuation segments.
    ProveApp,
    /// Generate a full end-to-end STARK proof with aggregation.
    ProveStark,
    /// deserialized app proofs and run STARK proof with aggregation.
    ProveStarkOnly,
    /// Generate a full end-to-end halo2 proof for EVM verifier.
    #[cfg(feature = "evm-verify")]
    ProveEvm,
    /// Generate input file only.
    MakeInput,
    /// Generate fixtures file for futher benchmarking.
    GenerateFixtures,
}

impl std::fmt::Display for BenchMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ExecuteHost => write!(f, "execute_host"),
            Self::Execute => write!(f, "execute"),
            Self::ExecuteMetered => write!(f, "execute_metered"),
            Self::ProveApp => write!(f, "prove_app"),
            Self::ProveStark => write!(f, "prove_stark"),
            #[cfg(feature = "evm-verify")]
            Self::ProveEvm => write!(f, "prove_evm"),
            Self::MakeInput => write!(f, "make_input"),
            Self::GenerateFixtures => write!(f, "generate_fixtures"),
            Self::ProveStarkOnly => write!(f, "prove_stark_only"),
        }
    }
}

/// The arguments for the host executable.
#[derive(Debug, Parser)]
pub struct HostArgs {
    /// The block number of the block to execute.
    #[clap(long)]
    block_number: u64,
    #[clap(flatten)]
    provider: ProviderArgs,

    /// The execution mode.
    #[clap(long, value_enum)]
    mode: BenchMode,

    /// Optional path to the directory containing cached client input. A new cache file will be
    /// created from RPC data if it doesn't already exist.
    #[clap(long)]
    cache_dir: Option<PathBuf>,
    /// The path to the CSV file containing the execution data.
    #[clap(long, default_value = "report.csv")]
    report_path: PathBuf,

    #[cfg(feature = "openvm-backend")]
    #[clap(flatten)]
    benchmark: BenchmarkCli,

    /// Optional path to the input file.
    #[arg(long)]
    pub input_path: Option<PathBuf>,

    /// Path to write the fixtures to. Only needed for mode=make_input
    #[arg(long)]
    pub fixtures_path: Option<PathBuf>,

    /// In make_input mode, this path is where the input JSON is written.
    #[arg(long)]
    pub generated_input_path: Option<PathBuf>,

    /// If specificed, the proof and other output is written to this dir.
    #[arg(long)]
    pub output_dir: Option<PathBuf>,

    /// If specified, loads the app proving key from this path.
    #[arg(long)]
    pub app_pk_path: Option<PathBuf>,

    /// If specified, loads the agg proving key from this path.
    #[arg(long)]
    pub agg_pk_path: Option<PathBuf>,

    #[arg(long, default_value_t = false)]
    pub skip_comparison: bool,

    /// only run specific shard id, which is for debug purpose
    #[arg(long)]
    pub shard_id: Option<u64>,

    /// app_proofs path when used in prove-stark-only mode
    #[arg(long)]
    pub app_proofs: Option<PathBuf>,
}

#[cfg(feature = "openvm-backend")]
fn write_versioned_proof(
    output_dir: &Path,
    block_number: u64,
    versioned_proof: VersionedVmStarkProof,
) -> eyre::Result<()> {
    let proof_path = output_dir.join(format!("{}_proof.json", block_number));
    let json = serde_json::to_vec_pretty(&versioned_proof)?;
    fs::write(&proof_path, json)?;
    println!("wrote proof json to {}", proof_path.display());
    Ok(())
}

fn handle_ceno_root_proof(
    output_dir: Option<&PathBuf>,
    block_number: u64,
    root_proof: &impl serde::Serialize,
) -> eyre::Result<()> {
    let proof_bytes = bincode::serde::encode_to_vec(root_proof, bincode::config::standard())?;
    println!(
        "ceno root proof size: {} bytes ({:.2} MiB)",
        proof_bytes.len(),
        proof_bytes.len() as f64 / (1024.0 * 1024.0)
    );

    if let Some(output_dir) = output_dir {
        fs::create_dir_all(output_dir)?;
        let proof_path = output_dir.join(format!("{}_root_proof.bin", block_number));
        fs::write(&proof_path, proof_bytes)?;
        println!("wrote ceno root proof to {}", proof_path.display());
    }

    Ok(())
}

fn read_object_from_file<T: serde::de::DeserializeOwned>(path: &Path) -> eyre::Result<T> {
    let bytes = fs::read(path)?;
    Ok(bitcode::deserialize(&bytes)?)
}

#[cfg(feature = "openvm-backend")]
pub fn reth_vm_config(_app_log_blowup: usize) -> SdkVmConfig {
    unimplemented!("only for openvm logic")
    // let mut config = toml::from_str::<AppConfig<SdkVmConfig>>(include_str!(
    //     "../../../bin/client-eth/openvm.toml"
    // ))
    // .unwrap()
    // .app_vm_config;
    // config.system.config = config
    //     .system
    //     .config
    //     .with_max_constraint_degree((1 << app_log_blowup) + 1)
    //     .with_public_values(32);
    // config
}

#[cfg(feature = "openvm-backend")]
pub const RETH_DEFAULT_APP_LOG_BLOWUP: usize = 1;
#[cfg(feature = "openvm-backend")]
pub const RETH_DEFAULT_LEAF_LOG_BLOWUP: usize = 1;

fn discover_workspace_root() -> PathBuf {
    if let Ok(path) = env::var("WORKSPACE_ROOT") {
        let pb = PathBuf::from(path);
        eprintln!("WORKSPACE_ROOT (env) = {}", pb.display());
        return pb;
    }

    if let Ok(metadata) = MetadataCommand::new().no_deps().exec() {
        let root = metadata.workspace_root.into_std_path_buf();
        eprintln!("WORKSPACE_ROOT (cargo-metadata) = {}", root.display());
        return root;
    }

    if let Ok(exe_path) = env::current_exe() {
        let mut dir =
            exe_path.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
        loop {
            if dir.join("Cargo.lock").exists() {
                eprintln!("WORKSPACE_ROOT (inferred from exe) = {}", dir.display());
                return dir;
            }
            if !dir.pop() {
                break;
            }
        }
    }

    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    eprintln!("WORKSPACE_ROOT fallback to cwd = {}", cwd.display());
    cwd
}

static WORKSPACE_ROOT: LazyLock<PathBuf> = LazyLock::new(discover_workspace_root);

fn setup() -> (Vec<u8>, Program, Platform) {
    let stack_size = 128 * 1024 * 1024;
    let heap_size = 128 * 1024 * 1024;
    println!("stack_size: {stack_size:#x}, heap_size: {heap_size:#x}");

    let elf_path = WORKSPACE_ROOT
        .join("bin")
        .join("ceno-client-eth")
        .join("target")
        .join("riscv32im-ceno-zkvm-elf")
        .join("release")
        .join("ceno-client-eth");
    let elf = std::fs::read(elf_path).unwrap();
    let program = Program::load_elf(&elf, u32::MAX).unwrap();
    let platform = setup_platform(Preset::Ceno, &program, stack_size, heap_size);
    (elf, program, platform)
}

pub const MAX_CYCLE_PER_SHARD: u64 = 1 << 29;

type CenoPcs = mpcs::Jagged<mpcs::Basefold<ff_ext::BabyBearExt4, mpcs::BasefoldRSParams>>;
type CenoBenchSdk = ceno_sdk::CenoSDK<ff_ext::BabyBearExt4, CenoPcs>;
type CenoBenchProof = ceno_zkvm::scheme::ZKVMProof<ff_ext::BabyBearExt4, CenoPcs>;

fn env_usize_or(name: &str, default: usize) -> usize {
    std::env::var(name).ok().and_then(|value| value.parse::<usize>().ok()).unwrap_or(default)
}

fn ceno_recursion_backend_label() -> &'static str {
    #[cfg(feature = "gpu")]
    {
        "gpu"
    }
    #[cfg(not(feature = "gpu"))]
    {
        "cpu"
    }
}

fn env_flag_enabled(name: &str) -> bool {
    matches!(
        std::env::var(name).as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes") | Ok("YES")
    )
}

fn init_ceno_agg_prover(sdk: &CenoBenchSdk) -> eyre::Result<ceno_sdk::CenoRecursionV2Prover> {
    sdk.init_agg_prover().map_err(|err| eyre::eyre!("{err:?}"))
}

fn init_tracing() {
    use tracing_forest::ForestLayer;
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

    let _ = tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .with(SpanMetricsLayer)
        .with(ForestLayer::default())
        .try_init();
}

pub async fn run_ceno_reth_benchmark(args: HostArgs) -> eyre::Result<()> {
    // Initialize the environment variables.
    dotenv::dotenv().ok();

    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info");
    }
    init_tracing();

    let client_input_from_path =
        args.input_path.as_ref().map(|path| try_load_input_from_path(path).unwrap());

    let client_input = if let Some(client_input_from_path) = client_input_from_path {
        client_input_from_path
    } else {
        let provider_config = args.provider.clone().into_provider().await?;
        match provider_config.chain_id {
            #[allow(non_snake_case)]
            CHAIN_ID_ETH_MAINNET => (),
            _ => {
                eyre::bail!("unknown chain ID: {}", provider_config.chain_id);
            }
        };
        let client_input_from_cache = try_load_input_from_cache(
            args.cache_dir.as_ref(),
            provider_config.chain_id,
            args.block_number,
        )?;

        match (client_input_from_cache, provider_config.rpc_url) {
            (Some(client_input_from_cache), _) => client_input_from_cache,
            (None, Some(rpc_url)) => {
                info!("calling rpc");
                // Cache not found but we have RPC
                // Setup the provider.
                let client =
                    RpcClient::builder().layer(RetryBackoffLayer::new(5, 1000, 100)).http(rpc_url);
                let provider = RootProvider::new(client);

                // Setup the host executor.
                let host_executor = HostExecutor::new(provider);

                info!("start host_executor");
                // Execute the host.
                let client_input =
                    host_executor.execute(args.block_number).await.expect("failed to execute host");
                info!("finish host_executor");

                if let Some(cache_dir) = args.cache_dir.as_ref() {
                    let input_folder =
                        cache_dir.join(format!("input/{}", provider_config.chain_id));
                    if !input_folder.exists() {
                        std::fs::create_dir_all(&input_folder)?;
                    }

                    let input_path = input_folder.join(format!("{}.bin", args.block_number));
                    let mut cache_file = std::fs::File::create(input_path)?;

                    bincode::serde::encode_into_std_write(
                        &client_input,
                        &mut cache_file,
                        bincode::config::standard(),
                    )?;
                }

                client_input
            }
            (None, None) => {
                eyre::bail!("cache not found and RPC URL not provided")
            }
        }
    };

    let (_, security_level) = default_backend_config();
    let max_num_variables = 26;
    let (_, program, platform) = setup();

    if matches!(args.mode, BenchMode::MakeInput) {
        let output_root = args
            .generated_input_path
            .clone()
            .unwrap_or_else(|| args.cache_dir.clone().unwrap_or_default());
        if output_root.as_os_str().is_empty() {
            eyre::bail!("generated_input_path or cache_dir must be provided in make_input mode");
        }
        let provider_config = args.provider.clone().into_provider().await?;
        let chain_id = provider_config.chain_id;
        let cache_dir = output_root.join(format!("input/{chain_id}"));
        if !cache_dir.exists() {
            std::fs::create_dir_all(&cache_dir)?;
        }
        let cache_path = cache_dir.join(format!("{}.bin", args.block_number));
        let mut cache_file = std::fs::File::create(cache_path)?;
        bincode::serde::encode_into_std_write(
            &client_input,
            &mut cache_file,
            bincode::config::standard(),
        )?;
        return Ok(());
    }

    #[cfg(feature = "gpu")]
    println!("CUDA Backend Enabled");

    let max_steps = usize::MAX;

    let max_cell_per_shard = std::env::var("CENO_MAX_CELL_PER_SHARD")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or((1 << 30) * 8 / 4 / 2);
    println!("ceno max_cell_per_shard: {max_cell_per_shard}");

    let default_l_skip = env_usize_or("CENO_REC_L_SKIP", ceno_sdk::DEFAULT_RECURSION_L_SKIP);
    let default_k_whir = env_usize_or("CENO_REC_K_WHIR", ceno_sdk::DEFAULT_RECURSION_K_WHIR);
    let leaf_l_skip = env_usize_or("CENO_REC_LEAF_L_SKIP", default_l_skip);
    let internal_l_skip = env_usize_or("CENO_REC_INTERNAL_L_SKIP", default_l_skip);
    let root_l_skip = env_usize_or("CENO_REC_ROOT_L_SKIP", default_l_skip);
    let leaf_n_stack = env_usize_or("CENO_REC_LEAF_N_STACK", 18);
    let internal_n_stack = env_usize_or("CENO_REC_INTERNAL_N_STACK", 18);
    let root_n_stack = env_usize_or("CENO_REC_ROOT_N_STACK", ceno_sdk::DEFAULT_RECURSION_N_STACK);
    let leaf_k_whir = env_usize_or("CENO_REC_LEAF_K_WHIR", default_k_whir);
    let internal_k_whir = env_usize_or("CENO_REC_INTERNAL_K_WHIR", default_k_whir);
    let root_k_whir = env_usize_or("CENO_REC_ROOT_K_WHIR", default_k_whir);
    println!(
        "ceno recursion params: leaf(l_skip={leaf_l_skip}, n_stack={leaf_n_stack}, k_whir={leaf_k_whir}), internal(l_skip={internal_l_skip}, n_stack={internal_n_stack}, k_whir={internal_k_whir}), root(l_skip={root_l_skip}, n_stack={root_n_stack}, k_whir={root_k_whir})"
    );
    let aggregation_options = ceno_sdk::recursion_aggregation_options(
        ceno_sdk::recursion_system_params(leaf_l_skip, leaf_n_stack, leaf_k_whir),
        ceno_sdk::recursion_system_params(internal_l_skip, internal_n_stack, internal_k_whir),
        ceno_sdk::recursion_system_params(root_l_skip, root_n_stack, root_k_whir),
    );

    let new_jagged_sdk = || -> eyre::Result<CenoBenchSdk> {
        let sdk_setup_start = std::time::Instant::now();
        let mut sdk = ceno_sdk::CenoSDK::new_with_app_config(
            program.clone(),
            platform.clone(),
            MultiProver::new(0, 1, max_cell_per_shard, MAX_CYCLE_PER_SHARD),
        );
        sdk.set_aggregation_options(aggregation_options.clone());
        let sdk_setup_elapsed = sdk_setup_start.elapsed();
        println!("ceno prove-stark sdk setup time: {sdk_setup_elapsed:?}");

        let base_prover_setup_start = std::time::Instant::now();
        sdk.init_base_prover(max_num_variables, security_level);
        let base_prover_setup_elapsed = base_prover_setup_start.elapsed();
        println!("ceno prove-stark base prover setup time: {base_prover_setup_elapsed:?}");
        info!("setup ceno jagged sdk done in {:?}", sdk_setup_elapsed + base_prover_setup_elapsed);
        Ok(sdk)
    };

    if args.agg_pk_path.is_some() {
        eyre::bail!("--agg-pk-path is not supported by ceno recursion v2 prove-stark");
    }

    let program_name = format!("reth.{}.block_{}", args.mode, args.block_number);
    let needs_ceno_sdk = matches!(
        args.mode,
        BenchMode::ProveApp |
            BenchMode::ProveStark |
            BenchMode::ProveStarkOnly |
            BenchMode::GenerateFixtures
    );
    let needs_ceno_agg = matches!(
        args.mode,
        BenchMode::ProveStark | BenchMode::ProveStarkOnly | BenchMode::GenerateFixtures
    );
    let ceno_recursion_backend = ceno_recursion_backend_label();
    let mut prebuilt_jagged_sdk = if needs_ceno_sdk { Some(new_jagged_sdk()?) } else { None };
    let mut prebuilt_agg_prover = if needs_ceno_agg {
        let recursion_setup_start = std::time::Instant::now();
        let sdk = prebuilt_jagged_sdk
            .as_ref()
            .expect("ceno sdk should be initialized before recursion setup");
        let agg_prover =
            info_span!("recursion.init_agg_prover").in_scope(|| init_ceno_agg_prover(sdk))?;
        let recursion_setup_elapsed = recursion_setup_start.elapsed();
        println!(
            "ceno prove-stark recursion setup time ({mode}): {recursion_setup_elapsed:?}",
            mode = ceno_recursion_backend
        );
        Some(agg_prover)
    } else {
        None
    };
    let needs_ceno_hints = matches!(args.mode, BenchMode::ProveApp | BenchMode::ProveStark);
    let mut prebuilt_hints = if needs_ceno_hints {
        let mut hints = CenoStdin::default();
        info_span!("app.hints").in_scope(|| write_ceno_client_input(&mut hints, &client_input))?;
        Some(hints)
    } else {
        None
    };
    #[cfg(all(feature = "aot", target_arch = "x86_64", target_os = "linux"))]
    if let (Some(ceno_sdk), Some(hints)) = (prebuilt_jagged_sdk.as_mut(), prebuilt_hints.as_ref()) {
        info_span!("sdk.prepare_preflight_aot").in_scope(|| ceno_sdk.prepare_preflight_aot(hints));
    }

    run_with_metric_collection("OUTPUT_PATH", || {
        info_span!("reth-block", block_number = args.block_number).in_scope(
            || -> eyre::Result<()> {
                // Run host execution for comparison
                // if !args.skip_comparison {
                let block_hash = info_span!("host.execute", group = program_name).in_scope(
                    || -> eyre::Result<_> {
                        let executor = ClientExecutor;
                        // Create a child span to get the group label propagated
                        let header = info_span!("client.execute").in_scope(|| {
                            executor.execute(ChainVariant::Mainnet, client_input.clone())
                        })?;
                        let block_hash =
                            info_span!("header.hash_slow").in_scope(|| header.hash_slow());
                        Ok(block_hash)
                    },
                )?;
                println!("block_hash (execute-host): {}", ToHexExt::encode_hex(&block_hash));
                // }

                // For ExecuteHost mode, only do host execution
                if matches!(args.mode, BenchMode::ExecuteHost) {
                    return Ok(());
                }

                // Execute for benchmarking:
                if !args.skip_comparison {
                    // let pvs = info_span!("sdk.execute", group = program_name)
                    //     .in_scope(|| sdk.execute(elf.clone(), stdin.clone()))?;
                    // let block_hash = pvs;
                    // println!("block_hash (execute): {}", ToHexExt::encode_hex(&block_hash));
                }

                match args.mode {
                    BenchMode::Execute => {}
                    BenchMode::ExecuteMetered => {
                        unimplemented!()
                        // let engine =
                        // DefaultStarkEngine::new(app_config.app_fri_params.fri_params);
                        // let (vm, _) = VirtualMachine::new_with_keygen(
                        //     engine,
                        //     SdkVmBuilder,
                        //     app_config.app_vm_config,
                        // )?;
                        // let executor_idx_to_air_idx = vm.executor_idx_to_air_idx();
                        // let interpreter =
                        //     vm.executor().metered_instance(&exe, &executor_idx_to_air_idx)?;
                        // let metered_ctx = vm.build_metered_ctx(&exe);
                        // let (segments, _) =
                        //     info_span!("interpreter.execute_metered", group = program_name)
                        //         .in_scope(|| interpreter.execute_metered(stdin, metered_ctx))?;
                        // println!("Number of segments: {}", segments.len());
                    }
                    BenchMode::ProveApp => {
                        let ceno_sdk = prebuilt_jagged_sdk
                            .take()
                            .expect("ceno sdk should be initialized before reth-block");
                        let hints = prebuilt_hints
                            .take()
                            .expect("ceno hints should be initialized before reth-block");
                        let pub_io_digest = unsafe {
                            core::mem::transmute::<[u8; 32], [u32; 8]>(block_hash.0)
                        };

                        let proofs = info_span!("app.prove").in_scope(|| {
                            ceno_sdk.generate_base_proof(
                                hints,
                                pub_io_digest,
                                max_steps,
                                args.shard_id.map(|v| v as usize),
                            )
                        });

                        if let Some(output_dir) = args.output_dir.as_ref() {
                            fs::create_dir_all(output_dir)?;
                            let mut path = output_dir.clone();
                            path.push("app_proof.bitcode");

                            fs::write(path, bitcode::serialize(&proofs)?)?;
                        };

                        let verifier = ceno_sdk.create_zkvm_verifier();
                        info_span!("app.verify").in_scope(|| match args.shard_id {
                            Some(_) => run_e2e_single_shard_debug_verify(
                                &verifier,
                                proofs
                                    .into_iter()
                                    .next()
                                    .expect("missing shard proof for debug verify"),
                                Some(0),
                                max_steps,
                            ),
                            None => {
                                run_e2e_full_trace_verify(&verifier, proofs, Some(0), max_steps)
                            }
                        });
                    }
                    BenchMode::ProveStark => {
                        let jagged_sdk = prebuilt_jagged_sdk
                            .take()
                            .expect("ceno sdk should be initialized before reth-block");
                        let agg_prover = prebuilt_agg_prover
                            .take()
                            .expect("ceno agg prover should be initialized before reth-block");
                        let hints = prebuilt_hints
                            .take()
                            .expect("ceno hints should be initialized before reth-block");
                        let pub_io_digest = unsafe {
                            core::mem::transmute::<[u8; 32], [u32; 8]>(block_hash.0)
                        };
                        let total_create_proof_start = std::time::Instant::now();
                        let app_prove_start = std::time::Instant::now();
                        let proofs = info_span!("app.prove").in_scope(|| {
                            jagged_sdk.generate_base_proof(
                                hints,
                                pub_io_digest,
                                max_steps,
                                args.shard_id.map(|v| v as usize),
                            )
                        });
                        let app_prove_elapsed = app_prove_start.elapsed();
                        println!("ceno prove-stark app create_proof time: {app_prove_elapsed:?}");

                        if let Some(output_dir) = args.output_dir.as_ref() {
                            fs::create_dir_all(output_dir)?;
                            let mut path = output_dir.clone();
                            path.push("app_proof.bitcode");
                            fs::write(path, bitcode::serialize(&proofs)?)?;
                        };

                        let timed_root_output = info_span!("recursion.compress_to_root_proof")
                            .in_scope(|| agg_prover.prove_with_root_vk_timed(&proofs))?;
                        let root_output = timed_root_output.root_output;
                        println!(
                            "ceno prove-stark recursion leaf aggregation time ({mode}): {:?}",
                            timed_root_output.timings.leaf_aggregation,
                            mode = ceno_recursion_backend
                        );
                        println!(
                            "ceno prove-stark recursion internal aggregation time ({mode}): {:?}",
                            timed_root_output.timings.internal_aggregation,
                            mode = ceno_recursion_backend
                        );
                        println!(
                            "ceno prove-stark recursion root proving time ({mode}): {:?}",
                            timed_root_output.timings.root_proving,
                            mode = ceno_recursion_backend
                        );
                        println!(
                            "ceno prove-stark recursion total create_proof time ({mode}): {:?}",
                            timed_root_output.timings.total_create_proof,
                            mode = ceno_recursion_backend
                        );

                        let root_verify_start = std::time::Instant::now();
                        info_span!("recursion.verify").in_scope(|| {
                            agg_prover
                                .verify_root_proof(&root_output.root_vk, &root_output.root_proof)
                                .expect("root proof verification failed");
                        });
                        let root_verify_elapsed = root_verify_start.elapsed();
                        println!("ceno prove-stark root verify time: {root_verify_elapsed:?}");

                        handle_ceno_root_proof(
                            args.output_dir.as_ref(),
                            args.block_number,
                            &root_output.root_proof,
                        )?;

                        let total_create_proof_elapsed = total_create_proof_start.elapsed();
                        println!(
                            "ceno prove-stark total create_proof time ({mode}): {total_create_proof_elapsed:?}",
                            mode = ceno_recursion_backend
                        );
                    }
                    BenchMode::ProveStarkOnly => {
                        let agg_prover = prebuilt_agg_prover
                            .take()
                            .expect("ceno agg prover should be initialized before reth-block");
                        let Some(app_proofs_path) = args.app_proofs.as_ref() else {
                            panic!("empty app_proofs_path")
                        };
                        let proofs: Vec<CenoBenchProof> = read_object_from_file(app_proofs_path)?;
                        if env_flag_enabled("CENO_VERIFY_APP_PROOF_BEFORE_RECURSION") {
                            let sdk = prebuilt_jagged_sdk
                                .as_ref()
                                .expect("ceno sdk should be initialized before app proof verify");
                            let verifier = sdk.create_zkvm_verifier();
                            let verify_proofs = proofs.clone();
                            info_span!("app.verify.loaded").in_scope(|| {
                                run_e2e_full_trace_verify(
                                    &verifier,
                                    verify_proofs,
                                    Some(0),
                                    max_steps,
                                )
                            });
                        }
                        let timed_root_output = info_span!("recursion.compress_to_root_proof")
                            .in_scope(|| agg_prover.prove_with_root_vk_timed(&proofs))?;
                        let root_output = timed_root_output.root_output;
                        println!(
                            "ceno prove-stark recursion leaf aggregation time ({mode}): {:?}",
                            timed_root_output.timings.leaf_aggregation,
                            mode = ceno_recursion_backend
                        );
                        println!(
                            "ceno prove-stark recursion internal aggregation time ({mode}): {:?}",
                            timed_root_output.timings.internal_aggregation,
                            mode = ceno_recursion_backend
                        );
                        println!(
                            "ceno prove-stark recursion root proving time ({mode}): {:?}",
                            timed_root_output.timings.root_proving,
                            mode = ceno_recursion_backend
                        );
                        println!(
                            "ceno prove-stark recursion total create_proof time ({mode}): {:?}",
                            timed_root_output.timings.total_create_proof,
                            mode = ceno_recursion_backend
                        );
                        info_span!("recursion.verify").in_scope(|| {
                            agg_prover
                                .verify_root_proof(&root_output.root_vk, &root_output.root_proof)
                                .expect("root proof verification failed");
                        });
                        handle_ceno_root_proof(
                            args.output_dir.as_ref(),
                            args.block_number,
                            &root_output.root_proof,
                        )?;
                    }
                    #[cfg(feature = "evm-verify")]
                    BenchMode::ProveEvm => {
                        let mut prover = sdk.evm_prover(elf)?.with_program_name(program_name);
                        let halo2_pk = sdk.halo2_pk();
                        tracing::info!(
                            "halo2_outer_k: {}",
                            halo2_pk.verifier.pinning.metadata.config_params.k
                        );
                        tracing::info!(
                            "halo2_wrapper_k: {}",
                            halo2_pk.wrapper.pinning.metadata.config_params.k
                        );
                        let proof = prover.prove_evm(stdin)?;
                        let block_hash = &proof.user_public_values;
                        println!("block_hash (prove_evm): {}", ToHexExt::encode_hex(block_hash));
                    }
                    BenchMode::GenerateFixtures => {
                        let jagged_sdk = prebuilt_jagged_sdk
                            .take()
                            .expect("ceno sdk should be initialized before reth-block");
                        let _app_pk = jagged_sdk.get_app_pk();
                        let agg_prover = prebuilt_agg_prover
                            .take()
                            .expect("ceno agg prover should be initialized before reth-block");
                        let _agg_vk = agg_prover.leaf_vk();

                        tracing::info!(
                            "ceno recursion v2 aggregation keys are initialized in-process"
                        );
                    }
                    _ => {
                        // This case is handled earlier and should not reach here
                        unreachable!();
                    }
                }

                Ok(())
            },
        )
    })?;
    Ok(())
}

#[cfg(feature = "openvm-backend")]
pub async fn run_reth_benchmark(args: HostArgs, openvm_client_eth_elf: &[u8]) -> eyre::Result<()> {
    // Initialize the environment variables.
    dotenv::dotenv().ok();

    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info");
    }
    init_tracing();

    // Parse the command line arguments.
    let mut args = args;

    let client_input_from_path =
        args.input_path.as_ref().map(|path| try_load_input_from_path(path).unwrap());

    let client_input = if let Some(client_input_from_path) = client_input_from_path {
        client_input_from_path
    } else {
        let provider_config = args.provider.into_provider().await?;
        match provider_config.chain_id {
            #[allow(non_snake_case)]
            CHAIN_ID_ETH_MAINNET => (),
            _ => {
                eyre::bail!("unknown chain ID: {}", provider_config.chain_id);
            }
        };
        let client_input_from_cache = try_load_input_from_cache(
            args.cache_dir.as_ref(),
            provider_config.chain_id,
            args.block_number,
        )?;

        match (client_input_from_cache, provider_config.rpc_url) {
            (Some(client_input_from_cache), _) => client_input_from_cache,
            (None, Some(rpc_url)) => {
                // Cache not found but we have RPC
                // Setup the provider.
                let client =
                    RpcClient::builder().layer(RetryBackoffLayer::new(5, 1000, 100)).http(rpc_url);
                let provider = RootProvider::new(client);

                // Setup the host executor.
                let host_executor = HostExecutor::new(provider);

                // Execute the host.
                let client_input =
                    host_executor.execute(args.block_number).await.expect("failed to execute host");

                if let Some(cache_dir) = args.cache_dir {
                    let input_folder =
                        cache_dir.join(format!("input/{}", provider_config.chain_id));
                    if !input_folder.exists() {
                        std::fs::create_dir_all(&input_folder)?;
                    }

                    let input_path = input_folder.join(format!("{}.bin", args.block_number));
                    let mut cache_file = std::fs::File::create(input_path)?;

                    bincode::serde::encode_into_std_write(
                        &client_input,
                        &mut cache_file,
                        bincode::config::standard(),
                    )?;
                }

                client_input
            }
            (None, None) => {
                eyre::bail!("cache not found and RPC URL not provided")
            }
        }
    };

    let mut stdin = StdIn::default();
    stdin.write(&client_input);
    info!("input loaded");

    if matches!(args.mode, BenchMode::MakeInput) {
        let words: Vec<u32> = openvm::serde::to_vec(&client_input).unwrap();
        let bytes: Vec<u8> = words.into_iter().flat_map(|w| w.to_le_bytes()).collect();
        let hex_bytes = String::from("0x01") + &hex::encode(&bytes);
        let input = json!({
            "input": [hex_bytes]
        });
        let input = serde_json::to_string(&input).unwrap();
        fs::write(args.generated_input_path.unwrap(), input)?;
        return Ok(());
    }

    let app_log_blowup = args.benchmark.app_log_blowup.unwrap_or(RETH_DEFAULT_APP_LOG_BLOWUP);
    args.benchmark.app_log_blowup = Some(app_log_blowup);
    let leaf_log_blowup = args.benchmark.leaf_log_blowup.unwrap_or(RETH_DEFAULT_LEAF_LOG_BLOWUP);
    args.benchmark.leaf_log_blowup = Some(leaf_log_blowup);

    #[cfg(feature = "cuda")]
    println!("CUDA Backend Enabled");

    let vm_config = reth_vm_config(app_log_blowup);
    let app_config = args.benchmark.app_config(vm_config.clone());
    let sdk = Sdk::new(app_config.clone())?
        .with_agg_config(args.benchmark.agg_config())
        .with_agg_tree_config(args.benchmark.agg_tree_config);

    if args.app_pk_path.is_some() != args.agg_pk_path.is_some() {
        eyre::bail!("app_pk_path and agg_pk_path must be provided together");
    }
    if let Some(app_pk_path) = args.app_pk_path {
        let app_pk: AppProvingKey<SdkVmConfig> = read_object_from_file(app_pk_path)?;
        let agg_pk_path = args.agg_pk_path.unwrap();
        let agg_pk: AggProvingKey = read_object_from_file(agg_pk_path)?;
        let vm_config_loaded = app_pk.app_vm_pk.vm_config.clone();
        let vm_config_json =
            serde_json::to_value(&vm_config).expect("failed to serialize vm_config to json value");
        let vm_config_loaded_json = serde_json::to_value(&vm_config_loaded)
            .expect("failed to serialize vm_config_loaded to json value");
        assert_eq!(
            vm_config_json, vm_config_loaded_json,
            "vm_config mismatch between runtime config and proving key"
        );
        sdk.set_app_pk(app_pk).map_err(|_| eyre::eyre!("failed to set app pk"))?;
        sdk.set_agg_pk(agg_pk).map_err(|_| eyre::eyre!("failed to set agg pk"))?;
    }

    let elf = Elf::decode(openvm_client_eth_elf, MEM_SIZE as u32)?;
    let exe = sdk.convert_to_exe(elf.clone())?;

    let program_name = format!("reth.{}.block_{}", args.mode, args.block_number);
    // NOTE: args.benchmark.app_config resets SegmentationLimits if max_segment_length is set
    args.benchmark.max_segment_length = None;

    run_with_metric_collection("OUTPUT_PATH", || {
        info_span!("reth-block", block_number = args.block_number).in_scope(
            || -> eyre::Result<()> {
                // Run host execution for comparison
                if !args.skip_comparison {
                    let block_hash = info_span!("host.execute", group = program_name).in_scope(
                        || -> eyre::Result<_> {
                            let executor = ClientExecutor;
                            // Create a child span to get the group label propagated
                            let header = info_span!("client.execute").in_scope(|| {
                                executor.execute(ChainVariant::Mainnet, client_input.clone())
                            })?;
                            let block_hash =
                                info_span!("header.hash_slow").in_scope(|| header.hash_slow());
                            Ok(block_hash)
                        },
                    )?;
                    println!("block_hash (execute-host): {}", ToHexExt::encode_hex(&block_hash));
                }

                // For ExecuteHost mode, only do host execution
                if matches!(args.mode, BenchMode::ExecuteHost) {
                    return Ok(());
                }

                // Execute for benchmarking:
                if !args.skip_comparison {
                    let pvs = info_span!("sdk.execute", group = program_name)
                        .in_scope(|| sdk.execute(elf.clone(), stdin.clone()))?;
                    let block_hash = pvs;
                    println!("block_hash (execute): {}", ToHexExt::encode_hex(&block_hash));
                }

                match args.mode {
                    BenchMode::Execute => {}
                    BenchMode::ExecuteMetered => {
                        let engine = DefaultStarkEngine::new(app_config.app_fri_params.fri_params);
                        let (vm, _) = VirtualMachine::new_with_keygen(
                            engine,
                            SdkVmBuilder,
                            app_config.app_vm_config,
                        )?;
                        let executor_idx_to_air_idx = vm.executor_idx_to_air_idx();
                        let interpreter =
                            vm.executor().metered_instance(&exe, &executor_idx_to_air_idx)?;
                        let metered_ctx = vm.build_metered_ctx(&exe);
                        let (segments, _) =
                            info_span!("interpreter.execute_metered", group = program_name)
                                .in_scope(|| interpreter.execute_metered(stdin, metered_ctx))?;
                        println!("Number of segments: {}", segments.len());
                    }
                    BenchMode::ProveApp => {
                        let mut prover = sdk.app_prover(elf)?.with_program_name(program_name);
                        let (_, app_vk) = sdk.app_keygen();
                        let proof = prover.prove(stdin)?;
                        verify_app_proof(&app_vk, &proof)?;
                    }
                    BenchMode::ProveStark => {
                        let mut prover = sdk.prover(elf)?.with_program_name(program_name);
                        let proof = prover.prove(stdin)?;
                        let block_hash = proof
                            .user_public_values
                            .iter()
                            .map(|pv| pv.as_canonical_u32() as u8)
                            .collect::<Vec<u8>>();
                        println!("block_hash (prove_stark): {}", ToHexExt::encode_hex(&block_hash));

                        if let Some(output_dir) = args.output_dir.as_ref() {
                            let versioned_proof = VersionedVmStarkProof::new(proof)?;
                            write_versioned_proof(output_dir, args.block_number, versioned_proof)?;
                        }
                    }
                    #[cfg(feature = "evm-verify")]
                    BenchMode::ProveEvm => {
                        let mut prover = sdk.evm_prover(elf)?.with_program_name(program_name);
                        let halo2_pk = sdk.halo2_pk();
                        tracing::info!(
                            "halo2_outer_k: {}",
                            halo2_pk.verifier.pinning.metadata.config_params.k
                        );
                        tracing::info!(
                            "halo2_wrapper_k: {}",
                            halo2_pk.wrapper.pinning.metadata.config_params.k
                        );
                        let proof = prover.prove_evm(stdin)?;
                        let block_hash = &proof.user_public_values;
                        println!("block_hash (prove_evm): {}", ToHexExt::encode_hex(block_hash));
                    }
                    BenchMode::GenerateFixtures => {
                        let mut prover = sdk.prover(elf)?.with_program_name(program_name);
                        let app_proof = prover.app_prover.prove(stdin)?;
                        let leaf_proofs = prover.agg_prover.generate_leaf_proofs(&app_proof)?;
                        let fixture_path = args.fixtures_path.unwrap();

                        let mut app_proof_path = fixture_path.clone();
                        app_proof_path.push("app_proof.bitcode");
                        fs::write(app_proof_path, bitcode::serialize(&app_proof)?)?;

                        let mut leaf_proofs_path = fixture_path.clone();
                        leaf_proofs_path.push("leaf_proofs.bitcode");
                        fs::write(leaf_proofs_path, bitcode::serialize(&leaf_proofs)?)?;

                        let mut app_pk_path = fixture_path.clone();
                        app_pk_path.push("app_pk.bitcode");
                        fs::write(app_pk_path, bitcode::serialize(sdk.app_pk())?)?;

                        let mut agg_pk_path = fixture_path.clone();
                        agg_pk_path.push("agg_pk.bitcode");
                        fs::write(agg_pk_path, bitcode::serialize(sdk.agg_pk())?)?;
                    }
                    _ => {
                        // This case is handled earlier and should not reach here
                        unreachable!();
                    }
                }

                Ok(())
            },
        )
    })?;
    Ok(())
}

fn try_load_input_from_cache(
    cache_dir: Option<&PathBuf>,
    chain_id: u64,
    block_number: u64,
) -> eyre::Result<Option<ClientExecutorInput>> {
    Ok(if let Some(cache_dir) = cache_dir {
        let cache_path = cache_dir.join(format!("input/{chain_id}/{block_number}.bin"));

        if cache_path.exists() {
            // TODO: prune the cache if invalid instead
            let mut cache_file = std::fs::File::open(cache_path)?;
            let client_input: ClientExecutorInput =
                bincode::serde::decode_from_std_read(&mut cache_file, bincode::config::standard())?;

            Some(client_input)
        } else {
            None
        }
    } else {
        None
    })
}

fn try_load_input_from_path(path: &PathBuf) -> eyre::Result<ClientExecutorInput> {
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
    if ext.eq_ignore_ascii_case("json") {
        let s = std::fs::read_to_string(path)?;
        let v: serde_json::Value = serde_json::from_str(&s)?;
        let arr = v
            .get("input")
            .and_then(|v| v.as_array())
            .ok_or_else(|| eyre::eyre!("invalid JSON: missing 'input' array"))?;
        let hex_str = arr
            .first()
            .and_then(|v| v.as_str())
            .ok_or_else(|| eyre::eyre!("invalid JSON: 'input[0]' must be string"))?;
        let stripped = hex_str.trim_start_matches("0x");
        let mut bytes = hex::decode(stripped)?;
        if let Some(1u8) = bytes.first().copied() {
            bytes.remove(0);
        }
        if bytes.len() % 4 != 0 {
            eyre::bail!("input bytes length must be multiple of 4");
        }
        #[cfg(feature = "openvm-backend")]
        {
            let input: ClientExecutorInput = openvm::serde::from_slice(&bytes).map_err(|e| {
                eyre::eyre!("failed to decode input words using openvm::serde: {e:?}")
            })?;
            Ok(input)
        }
        #[cfg(not(feature = "openvm-backend"))]
        {
            eyre::bail!("JSON input decoding requires the openvm-backend feature")
        }
    } else {
        let mut file = std::fs::File::open(path)?;
        let client_input: ClientExecutorInput =
            bincode::serde::decode_from_std_read(&mut file, bincode::config::standard())?;
        Ok(client_input)
    }
}
