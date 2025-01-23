use anyhow::Result;
use futures::{AsyncBufReadExt, TryStreamExt};
use k8s_openapi::api::core::v1::Pod;
use kube::{
    api::{Api, ListParams, LogParams},
    Client,
};
use regex::Captures;
use regex::Regex;
use serde::Serialize;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::watch::{Receiver, Sender};
use tokio::sync::{watch, RwLock};
use tokio::time::{sleep, Duration};
use crate::tlds::TLDS;
use crate::config::CONFIG;

#[derive(Serialize)]
struct DnsData {
    internal: HashMap<String, Vec<String>>,
    external: HashMap<String, Vec<String>>,
}

#[derive(Clone)]
pub struct LogAnalyzer {
    client: Client,
    sender: Sender<String>,
    receiver: Receiver<String>,
}

impl LogAnalyzer {
    pub async fn new() -> Result<Self> {
        let (sender, receiver) = watch::channel(String::from("asd"));
        Ok(Self {
            client: Client::try_default().await?,
            sender,
            receiver,
        })
    }

    pub async fn get_update(&mut self) -> Result<String> {
        self.receiver.changed().await?;
        Ok(self.receiver.borrow().clone())
    }

    fn extract_domain_name(query_name: &str, response_code: &str) -> Option<(String, bool)> {
        let query = query_name.trim_end_matches('.');
        
        if query.ends_with(".svc.cluster.local") && response_code == "NOERROR" {
            Some((query.split('.').next().unwrap_or("unknown").to_string(), true))
        } else if TLDS.iter().any(|tld| query.to_lowercase().ends_with(&format!(".{}", tld))) 
               && response_code == "NOERROR" {
            Some((query.to_string(), false))  
        } else {
            None
        }
     }

    pub async fn analyze_loop(&self) -> Result<()> {
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &CONFIG.coredns_ns);
        let coredns_pod = pods
            .list(&kube::api::ListParams::default().labels(&CONFIG.coredns_label_selector))
            .await?
            .items
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("CoreDNS pod not found"))?;

        // Unwrap ok need pod to continue
        let pod_name = coredns_pod.metadata.name.unwrap();
        log::info!("Found CoreDNS pod: {}", pod_name);

        let lp = LogParams {
            container: Some("coredns".to_string()),
            follow: true,
            tail_lines: Some(1),
            ..Default::default()
        };

        let mut logs = pods.log_stream(&pod_name, &lp).await?.lines();
        let re = Regex::new(
            r#"\[INFO\] ([\d.:]+) - \d+ "([\w]+) IN ([\w.-]+) udp \d+ [\w]+ \d+" (\w+)"#,
        )?;

        let internal_map: Arc<RwLock<HashMap<String, Vec<String>>>> =
            Arc::new(RwLock::new(HashMap::new()));
        let external_map: Arc<RwLock<HashMap<String, Vec<String>>>> =
            Arc::new(RwLock::new(HashMap::new()));

        let internal_clone = internal_map.clone();
        let external_clone = external_map.clone();
        let client = self.client.clone();
        let sender = self.sender.clone();

        tokio::spawn(async move {
            loop {
                let log = logs.try_next().await;
                let line = match log {
                    Ok(log) => match log {
                        Some(line) => line,
                        None => continue,
                    },
                    Err(err) => {
                        log::error!("{}", err);
                        continue;
                    }
                };

                if let Some(captures) = re.captures(&line) {
                    let (client_ip, _query_type, query_name, response_code) = match parse_infos(captures) {
                        Some(res) => res,
                        None => continue,
                    };
                 
                    if let Some((domain_name, is_internal)) = Self::extract_domain_name(query_name, response_code) {
                        let pod_name = resolve_pod(&client, client_ip).await;
                        let map_to_update = if is_internal { &internal_clone } else { &external_clone };
                        
                        map_to_update
                            .write()
                            .await
                            .entry(domain_name)
                            .or_insert_with(Vec::new)
                            .push(pod_name);
                    }
                 }
            }
        });

        tokio::spawn(async move {
            loop {
                let dns_data = DnsData {
                    internal: (*internal_map.read().await).clone(),
                    external: (*external_map.read().await).clone(),
                };
                match serde_json::to_string(&dns_data) {
                    Ok(msg) => _ = sender.send(msg),
                    Err(err) => log::error!("Error: {}", err),
                };

                sleep(Duration::from_secs(2)).await;
            }
        });

        Ok(())
    }
}

fn parse_infos(captures: Captures<'_>) -> Option<(&str, &str, &str, &str)> {
    return Some((
        captures.get(1)?.as_str(),
        captures.get(2)?.as_str(),
        captures.get(3)?.as_str(),
        captures.get(4)?.as_str(),
    ));
}

async fn resolve_pod(client: &Client, ip: &str) -> String {
    let pods: Api<Pod> = Api::all(client.clone());
    let lp = ListParams::default().fields(&format!(
        "status.podIP={}",
        ip.split(":").collect::<Vec<_>>()[0]
    ));

    match pods.list(&lp).await {
        Ok(pod_list) => pod_list
            .items
            .into_iter()
            .next()
            .and_then(|pod| pod.metadata.name)
            .unwrap_or_else(|| "unknown".to_string()),
        Err(_) => "unknown".to_string(),
    }
}
