use openengine_cluster_server::stdio::serve_stdio;
use openengine_cluster_server::{ConnectionContext, Dispatcher};
use openengine_cluster_testkit::EmptyBackend;

#[tokio::main]
async fn main() {
    let dispatcher = Dispatcher::new(EmptyBackend, ConnectionContext::default());
    if let Err(error) = serve_stdio(dispatcher).await {
        eprintln!("cluster protocol stdio server failed: {error}");
        std::process::exit(1);
    }
}
