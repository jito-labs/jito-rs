use clap::Parser;
use jito_block_engine_proxy_server::json_rpc::server::ProxyServerImpl;
use std::fmt::Debug;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// port to bind to for the searcher json rpc service
    #[arg(long, env)]
    proxy_json_rpc_port: u16,

    /// Redis connection string. Eg "redis://127.0.0.1/"
    #[arg(long, env)]
    json_rpc_url: String,

    /// URL of the RPC server to connect to for HTTP requests
    #[arg(long, env)]
    key_pair_path: String,
}

#[tokio::main]
async fn main() {
    let args: Args = Args::parse();

    // Start the json rpc server
    let hdl = ProxyServerImpl::run(
        args.json_rpc_url,
        args.key_pair_path,
        args.proxy_json_rpc_port,
    )
    .await;
    hdl.await.expect("Proxy server is shutting down");
}
