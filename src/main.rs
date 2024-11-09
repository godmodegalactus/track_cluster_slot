use std::{collections::HashMap, env, time::Duration};

use futures::StreamExt;
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::commitment_config::CommitmentConfig;
use tokio::{fs::File, io::AsyncReadExt};
use yellowstone_grpc_proto::geyser::{SubscribeRequest, SubscribeRequestFilterSlots};


#[derive(Clone, Debug, serde_derive::Deserialize)]
#[repr(C)]
pub enum SourceType {
    Rpc,
    Grpc,
}

#[derive(Clone, Debug, serde_derive::Deserialize)]
pub struct SourceConfig {
    pub name: String,
    pub url: String,
    pub token: Option<String>,
    pub source_type: SourceType
}

#[derive(Clone, Debug, serde_derive::Deserialize)]
pub struct Config {
    pub sources : Vec<SourceConfig>,
}

impl Config {
    pub async fn load(path: &String) -> Result<Config, anyhow::Error> {
        log::info!("opening file at path :{path:?}");
        let mut file = File::open(path).await?;
        let mut contents = String::new();
        log::info!("Reading file");
        file.read_to_string(&mut contents).await?;
        log::info!("File read");
        let res = match toml::from_str(&contents) {
            Ok(c) => Ok(c),
            Err(e) => Err(anyhow::Error::new(e)),
        };
        log::info!("Contents read");
        res
    }
}

async fn run_rpc_source(config: &SourceConfig) {
    let rpc_client = RpcClient::new(config.url.clone());
    // let slot_finalized = rpc_client.get_slot_with_commitment(CommitmentConfig::finalized()).await.unwrap();
    // let slot_confirmed = rpc_client.get_slot_with_commitment(CommitmentConfig::confirmed()).await.unwrap();
    let mut slot_processed_old = rpc_client.get_slot_with_commitment(CommitmentConfig::processed()).await.unwrap();
    let mut interval = tokio::time::interval(Duration::from_millis(200));
    interval.tick().await;
    loop {
        interval.tick().await;
        if let Ok(slot_processed) = rpc_client.get_slot_with_commitment(CommitmentConfig::processed()).await {
            if slot_processed > slot_processed_old {
                slot_processed_old = slot_processed;
                println!("{} has update processed slot {}", config.name, slot_processed);
            }
        } else {
            log::warn!("Error fetching slot for {}", config.name);
        }
    }
}

async fn run_grpc_source(config: &SourceConfig) {
    let client_builder = yellowstone_grpc_client::GeyserGrpcBuilder::from_shared(config.url.clone()).unwrap();
    let client_builder = client_builder.x_token(config.token.clone()).unwrap();
    let mut client = client_builder.connect().await.unwrap();

    let slot_filter = SubscribeRequestFilterSlots {
        filter_by_commitment: None,
    };

    let mut map_slot_filter = HashMap::new();
    map_slot_filter.insert("slot_filter".to_string(), slot_filter);
    let mut s = client.subscribe_once(SubscribeRequest {
        slots: map_slot_filter,
        ..Default::default()
    }).await.unwrap();

    while let Some(m_res) = s.next().await {
        let Ok(message) = m_res else {
            log::warn!("Error on message {}", config.name);
            continue;
        };

        let Some(update) = message.update_oneof else {
            log::warn!("unknown update");
            continue;
        };

        match update {
            yellowstone_grpc_proto::geyser::subscribe_update::UpdateOneof::Slot(subscribe_update_slot) => {
                if subscribe_update_slot.status == 0 {
                    println!("{} has update processed slot {}", config.name, subscribe_update_slot.slot);
                }
            },
            _ => {
                // do nothing
            }
        }
    }
}

pub fn spawn_source(config: &SourceConfig) -> tokio::task::JoinHandle<()> {
    let config = config.clone();
    tokio::spawn(async move {
        match config.source_type {
            SourceType::Rpc => run_rpc_source(&config).await,
            SourceType::Grpc => run_grpc_source(&config).await,
        }
        log::error!("Source stopped : {}", config.name);
    })
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let args: Vec<String> = env::args().collect();
    let config_file = args.get(1).cloned().unwrap_or("config.toml".to_string());

    let config : Config = Config::load(&config_file).await?;
    let tasks = config.sources.iter().map(|conf| spawn_source(conf));

    let _ = futures::future::select_all(tasks).await;
    Ok(())
}
