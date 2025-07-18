
use crate::models::{ClusterInfo, NodeInfo, PodInfo, ContainerInfo};
use k8s_openapi::api::core::v1::{Node, Pod};
use kube::{
    api::{ListParams, ResourceExt},
    Client,
};
use std::collections::HashMap;
use log::{error};

pub async fn fetch_cluster_data(name: String, client: Client) -> Option<ClusterInfo> {
    let nodes_api: kube::Api<Node> = kube::Api::all(client.clone());
    let pods_api: kube::Api<Pod> = kube::Api::all(client);

    let lp = ListParams::default();
    let (nodes_res, pods_res) = tokio::join!(
        nodes_api.list(&lp),
        pods_api.list(&lp)
    );

    match (nodes_res, pods_res) {
        (Ok(nodes), Ok(pods)) => {
            let mut pods_by_node: HashMap<String, Vec<Pod>> = HashMap::new();
            for pod in pods.items {
                if let Some(node_name) = &pod.spec.as_ref().and_then(|s| s.node_name.clone()) {
                    pods_by_node.entry(node_name.clone()).or_default().push(pod);
                }
            }
            let total_pod_count = pods_by_node.values().map(|v| v.len()).sum();
            let nodes_info: Vec<NodeInfo> = nodes.items.iter().map(|node| {
                let node_name = node.name_any();
                let node_pods_vec = pods_by_node.get(&node_name).cloned().unwrap_or_default();
                let mut container_count_for_node = 0;
                let node_pods_info: Vec<PodInfo> = node_pods_vec.iter().map(|p| {
                    let containers: Vec<ContainerInfo> = p.spec.as_ref().map(|s| s.containers.iter().map(|c| ContainerInfo {
                        name: c.name.clone(), image: c.image.clone().unwrap_or_default(),
                    }).collect()).unwrap_or_default();
                    container_count_for_node += containers.len();
                    PodInfo { name: p.name_any(), namespace: p.namespace().unwrap_or_default(), node_name: node_name.clone(), labels: p.labels().clone(), containers, cluster_name: name.clone() }
                }).collect();
                
                let node_status = node.status.as_ref();
                let node_info_details = node_status.and_then(|s| s.node_info.as_ref());
                let node_labels = node.labels().clone();

                let gpu_model = node_labels.get("nvidia.com/gpu.product").cloned().unwrap_or_else(|| "N/A".to_string());
                let gpu_count = node_status.and_then(|s| s.capacity.as_ref()).and_then(|c| c.get("nvidia.com/gpu").map(|q| q.0.clone())).unwrap_or_else(|| "0".to_string());
                let mut mig_devices = HashMap::new();
                if let Some(capacity) = node_status.and_then(|s| s.capacity.as_ref()) {
                    for (key, value) in capacity {
                        if key.starts_with("nvidia.com/mig-") {
                            let mig_profile = key.strip_prefix("nvidia.com/mig-").unwrap_or(key);
                            mig_devices.insert(mig_profile.to_string(), value.0.clone());
                        }
                    }
                }
                
                NodeInfo {
                    name: node_name, labels: node.labels().clone(), pods: node_pods_info, pod_count: node_pods_vec.len(), container_count: container_count_for_node, cluster_name: name.clone(),
                    os_image: node_info_details.map_or_else(|| "N/A".to_string(), |ni| ni.os_image.clone()),
                    kubelet_version: node_info_details.map_or_else(|| "N/A".to_string(), |ni| ni.kubelet_version.clone()),
                    architecture: node_info_details.map_or_else(|| "N/A".to_string(), |ni| ni.architecture.clone()),
                    capacity_cpu: node_status.and_then(|s| s.capacity.as_ref()).and_then(|c| c.get("cpu").map(|q| q.0.clone())).unwrap_or_else(|| "N/A".to_string()),
                    capacity_memory: node_status.and_then(|s| s.capacity.as_ref()).and_then(|c| c.get("memory").map(|q| q.0.clone())).unwrap_or_else(|| "N/A".to_string()),
                    gpu_model, gpu_count, mig_devices,
                }
            }).collect();
            Some(ClusterInfo { name: name.clone(), node_count: nodes.items.len(), pod_count: total_pod_count, nodes: nodes_info })
        }
        (Err(e), _) => { error!("[{}] 노드 조회 실패: {}", name, e); None },
        (_, Err(e)) => { error!("[{}] 파드 조회 실패: {}", name, e); None },
    }
}
