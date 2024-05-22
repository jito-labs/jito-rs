# Jito proxy server for json rpc

The sample proxy server can be used to sign and send the payload to the Jito json rpc endpoint.

Expected environment variables are set in the config/.env file

The binary can be run by itself by providing the parameters
  --proxy-json-rpc-port <PROXY_JSON_RPC_PORT>
  --json-rpc-url <JSON_RPC_URL>
  --key-pair-path <KEY_PAIR_PATH>

## Running as a container

The server can be run as a container using the provided script, run_proxy_server.sh.

```shell
# starts the proxy server.
# The script reads env values from config/.env. Modify as necessary
./run_proxy_server
```

## Stopping the container

The container can be stopped and resources removed using, stop_proxy_server.sh.

```shell
# starts the proxy server.
# The script reads env values from config/.env. Modify as necessary
./stop_proxy_server
```

## Running the binary

The binary can be run by itself by providing the parameters
  --proxy-json-rpc-port <PROXY_JSON_RPC_PORT>
  --json-rpc-url <JSON_RPC_URL>
  --key-pair-path <KEY_PAIR_PATH>

```shell
# starts the proxy server.
# You can also source the env variables and pass them as parameters as well
cargo run --proxy-json-rpc-port 8080 --json-rpc-url url --key-pair-path
```

## Use curl or similar to send request

```shell
curl 'http://127.0.0.1:8080/api/v1/bundles' -POST -d '{"jsonrpc": "2.0", "method": "getTipAccounts", "params": [], "id": 1}' -H 'Content-Type: application/json
```
