use super::*;
use quai_sdk::rpc::Transport;
pub async fn run() -> Result<(), Box<dyn Error>> {
    let t = HttpTransport::new(HttpConfig::default())?;
    let e = quai_sdk::Endpoint::parse("https://orchard.rpc.quai.network/cyprus1")?;
    let hash = "0x001b00cb6d02ec92042b0adf667bc9302ffe5769756341c63d78fa377ca1402f";
    let head = t
        .request(&e, "quai_blockNumber", serde_json::json!([]))
        .await?;
    for (method, params) in [
        ("web3_clientVersion", serde_json::json!([])),
        ("quai_getTransactionByHash", serde_json::json!([hash])),
        ("quai_getTransactionReceipt", serde_json::json!([hash])),
        (
            "quai_getCode",
            serde_json::json!([quai_sdk::wrappers::WQUAI_ORCHARD_ADDRESS, head]),
        ),
        (
            "quai_getCode",
            serde_json::json!([quai_sdk::wrappers::WQI_ADDRESS, head]),
        ),
        (
            "quai_getOutpointsByAddress",
            serde_json::json!(["0x00f884B61A1B0a1D456D547801B30561d85E7A2D"]),
        ),
    ] {
        match t.request(&e, method, params.clone()).await {
            Ok(mut v) => {
                if let Some(o) = v.as_object_mut() {
                    o.remove("logsBloom");
                }
                if method == "quai_getCode" {
                    let s = v.as_str().ok_or("code response")?;
                    v = serde_json::json!({"codeBytes":s.strip_prefix("0x").ok_or("code hex")?.len()/2});
                }
                println!(
                    "{}",
                    serde_json::json!({"method":method,"params":params,"head":head,"result":v})
                );
            }
            Err(err) => eprintln!("{method}: {err}"),
        }
    }
    Ok(())
}
