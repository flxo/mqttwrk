//! Tool to vet mqtt brokers. The goal here is to test and benchmark
//! mqtt brokers for features, robustness, performance and scalability
//!
//! Goals of the test suite
//!
//! - Spawn n clients with publish and subscribe on the same topic (and report thoughput and latencies)
//! - Spawn n clinets with publishes and 1 subscription to pull all the data (used to simulate a sink in the cloud)
//! - Offline messaging
//! - Halfopen connection detection
//!

#[global_allocator]
static ALLOC: jemallocator::Jemalloc = jemallocator::Jemalloc;

#[macro_use]
extern crate log;

use std::sync::Arc;

use futures;
use tokio::task;
use tokio::sync::Barrier;
use argh::FromArgs;
use futures::stream::StreamExt;
use async_channel;

mod connection;
use hdrhistogram::Histogram;

#[derive(FromArgs)]
/// Reach new heights.
struct Config {
    /// number of connections
    #[argh(option, short = 'c', default = "1")]
    connections: usize,

    /// size of payload
    #[argh(option, short = 'm', default = "100")]
    payload_size: usize,

    /// number of messages
    #[argh(option, short = 'n', default = "1_000_000")]
    count: usize,

    /// server
    #[argh(option, short = 'h', default = "String::from(\"localhost\")")]
    server: String,

    /// port
    #[argh(option, short = 'p', default = "1883")]
    port: u16,

    /// keep alive
    #[argh(option, short = 'k', default = "10")]
    keep_alive: u16,

    /// max inflight messages
    #[argh(option, short = 'i', default = "100")]
    max_inflight: u16,

    /// path to PEM encoded x509 ca-chain file
    #[argh(option, short = 'R')]
    ca_file: Option<String>,

    /// path to PEM encoded x509 client cert file.
    #[argh(option, short = 'C')]
    client_cert: Option<String>,

    /// path to PEM encoded client key file
    #[argh(option, short = 'K')]
    client_key: Option<String>,

    /// connection_timeout
    #[argh(option, short = 't', default = "5")]
    conn_timeout: u64,

    /// qos, default 1
    #[argh(option, short = 'q', default = "1")]
    qos: i16,

    /// number of publishers, default 1
    #[argh(option, short = 'x', default = "1")]
    publishers: usize,

    /// number of subscribers, default 1
    #[argh(option, short = 'y', default = "1")]
    subscribers: usize,

    /// delay in between each request in secs
    #[argh(option, short = 'd', default = "0")]
    delay: u64,
}


#[tokio::main(core_threads = 4)]
async fn main() {
    pretty_env_logger::init();

    let config: Config = argh::from_env();
    let config = Arc::new(config);
    let barrier = Arc::new(Barrier::new(config.connections));
    let mut handles = futures::stream::FuturesUnordered::new();
    let (mut tx, mut rx) = async_channel::bounded::<Histogram::<u64>>(123);

    // We synchronously finish connections and subscriptions and then spawn connection
    // start to perform publishes concurrently. This simplifies 2 things
    // * Creating too many connections wouldn't lead to `Elapsed` error because
    //   broker accepts connections sequentially
    // * We don't have to synchronize all subscription with a barrier because
    //   subscriptions shouldn't happen after publish to prevent wrong incoming
    //   publish count
    for i in 0..config.connections {
        // let hist_channel = tx.clone();
        let mut connection = match connection::Connection::new(i, config.clone(), tx.clone()).await {
            Ok(c) => c,
            Err(e) => {
                error!("Device = {}, Error = {:?}", i, e);
                return
            }
        };


        let barrier = barrier.clone();
        handles.push(task::spawn(async move { 
            connection.start(barrier).await 
        }));
    }

    let mut cnt = 0;
    let mut hist = Histogram::<u64>::new(4).unwrap();

    loop {
        if handles.next().await.is_none() {
            break
        }

        // TODO Collect histograms
        if let Ok(h) = rx.try_recv() {
            cnt += 1;
            hist.add(h).unwrap();
        }
        if cnt == config.connections{
            break;
        }
    }
}
