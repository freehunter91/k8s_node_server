use crate::models::{ClusterInfo, NodeInfo, PodInfo, ContainerInfo};
use std::collections::{BTreeMap, HashMap};

pub fn generate_dummy_clusters() -> Vec<ClusterInfo> {
    vec![
        ClusterInfo {
            name: "production-cluster-01".to_string(),
            node_count: 3,
            pod_count: 25,
            nodes: vec![
                NodeInfo {
                    name: "gpu-node-01".to_string(),
                    cluster_name: "production-cluster-01".to_string(),
                    os_image: "Ubuntu 20.04.6 LTS".to_string(),
                    kubelet_version: "v1.28.2".to_string(),
                    architecture: "amd64".to_string(),
                    capacity_cpu: "32".to_string(),
                    capacity_memory: "128Gi".to_string(),
                    gpu_model: "NVIDIA A100-SXM4-80GB".to_string(),
                    gpu_count: "8".to_string(),
                    pod_count: 12,
                    container_count: 28,
                    labels: {
                        let mut labels = BTreeMap::new();
                        labels.insert("kubernetes.io/arch".to_string(), "amd64".to_string());
                        labels.insert("kubernetes.io/os".to_string(), "linux".to_string());
                        labels.insert("node.kubernetes.io/instance-type".to_string(), "gpu.8xlarge".to_string());
                        labels.insert("topology.kubernetes.io/zone".to_string(), "us-west-2a".to_string());
                        labels.insert("nvidia.com/gpu.present".to_string(), "true".to_string());
                        labels.insert("nvidia.com/gpu.count".to_string(), "8".to_string());
                        labels
                    },
                    mig_devices: {
                        let mut mig = HashMap::new();
                        mig.insert("1g.10gb".to_string(), "4".to_string());
                        mig.insert("2g.20gb".to_string(), "2".to_string());
                        mig.insert("3g.40gb".to_string(), "1".to_string());
                        mig
                    },
                    pods: vec![
                        PodInfo {
                            name: "ml-training-job-7f8g9h".to_string(),
                            namespace: "ml-workloads".to_string(),
                            node_name: "gpu-node-01".to_string(),
                            cluster_name: "production-cluster-01".to_string(),
                            labels: {
                                let mut labels = BTreeMap::new();
                                labels.insert("app".to_string(), "pytorch-training".to_string());
                                labels.insert("version".to_string(), "v2.1.0".to_string());
                                labels.insert("tier".to_string(), "compute".to_string());
                                labels
                            },
                            containers: vec![
                                ContainerInfo {
                                    name: "pytorch-trainer".to_string(),
                                    image: "pytorch/pytorch:2.1.0-cuda11.8-cudnn8-runtime".to_string(),
                                },
                                ContainerInfo {
                                    name: "data-loader".to_string(),
                                    image: "busybox:1.36".to_string(),
                                },
                            ],
                        },
                        PodInfo {
                            name: "inference-server-a1b2c3".to_string(),
                            namespace: "inference".to_string(),
                            node_name: "gpu-node-01".to_string(),
                            cluster_name: "production-cluster-01".to_string(),
                            labels: {
                                let mut labels = BTreeMap::new();
                                labels.insert("app".to_string(), "triton-inference".to_string());
                                labels.insert("version".to_string(), "v23.04".to_string());
                                labels.insert("tier".to_string(), "serving".to_string());
                                labels
                            },
                            containers: vec![
                                ContainerInfo {
                                    name: "triton-server".to_string(),
                                    image: "nvcr.io/nvidia/tritonserver:23.04-py3".to_string(),
                                },
                            ],
                        },
                    ],
                },
                NodeInfo {
                    name: "gpu-node-02".to_string(),
                    cluster_name: "production-cluster-01".to_string(),
                    os_image: "Ubuntu 20.04.6 LTS".to_string(),
                    kubelet_version: "v1.28.2".to_string(),
                    architecture: "amd64".to_string(),
                    capacity_cpu: "32".to_string(),
                    capacity_memory: "128Gi".to_string(),
                    gpu_model: "NVIDIA V100-SXM2-32GB".to_string(),
                    gpu_count: "4".to_string(),
                    pod_count: 8,
                    container_count: 15,
                    labels: {
                        let mut labels = BTreeMap::new();
                        labels.insert("kubernetes.io/arch".to_string(), "amd64".to_string());
                        labels.insert("kubernetes.io/os".to_string(), "linux".to_string());
                        labels.insert("node.kubernetes.io/instance-type".to_string(), "gpu.4xlarge".to_string());
                        labels.insert("topology.kubernetes.io/zone".to_string(), "us-west-2b".to_string());
                        labels.insert("nvidia.com/gpu.present".to_string(), "true".to_string());
                        labels.insert("nvidia.com/gpu.count".to_string(), "4".to_string());
                        labels
                    },
                    mig_devices: {
                        let mut mig = HashMap::new();
                        mig.insert("1g.5gb".to_string(), "2".to_string());
                        mig.insert("2g.10gb".to_string(), "1".to_string());
                        mig
                    },
                    pods: vec![
                        PodInfo {
                            name: "jupyter-notebook-x9y8z7".to_string(),
                            namespace: "research".to_string(),
                            node_name: "gpu-node-02".to_string(),
                            cluster_name: "production-cluster-01".to_string(),
                            labels: {
                                let mut labels = BTreeMap::new();
                                labels.insert("app".to_string(), "jupyter".to_string());
                                labels.insert("user".to_string(), "data-scientist-01".to_string());
                                labels.insert("tier".to_string(), "development".to_string());
                                labels
                            },
                            containers: vec![
                                ContainerInfo {
                                    name: "jupyter-server".to_string(),
                                    image: "jupyter/tensorflow-notebook:latest".to_string(),
                                },
                            ],
                        },
                    ],
                },
                NodeInfo {
                    name: "worker-node-01".to_string(),
                    cluster_name: "production-cluster-01".to_string(),
                    os_image: "Ubuntu 20.04.6 LTS".to_string(),
                    kubelet_version: "v1.28.2".to_string(),
                    architecture: "amd64".to_string(),
                    capacity_cpu: "16".to_string(),
                    capacity_memory: "64Gi".to_string(),
                    gpu_model: "N/A".to_string(),
                    gpu_count: "0".to_string(),
                    pod_count: 5,
                    container_count: 12,
                    labels: {
                        let mut labels = BTreeMap::new();
                        labels.insert("kubernetes.io/arch".to_string(), "amd64".to_string());
                        labels.insert("kubernetes.io/os".to_string(), "linux".to_string());
                        labels.insert("node.kubernetes.io/instance-type".to_string(), "c5.4xlarge".to_string());
                        labels.insert("topology.kubernetes.io/zone".to_string(), "us-west-2c".to_string());
                        labels
                    },
                    mig_devices: HashMap::new(),
                    pods: vec![
                        PodInfo {
                            name: "web-frontend-deployment-7d8e9f".to_string(),
                            namespace: "default".to_string(),
                            node_name: "worker-node-01".to_string(),
                            cluster_name: "production-cluster-01".to_string(),
                            labels: {
                                let mut labels = BTreeMap::new();
                                labels.insert("app".to_string(), "web-frontend".to_string());
                                labels.insert("version".to_string(), "v1.2.3".to_string());
                                labels.insert("tier".to_string(), "frontend".to_string());
                                labels
                            },
                            containers: vec![
                                ContainerInfo {
                                    name: "nginx".to_string(),
                                    image: "nginx:1.21".to_string(),
                                },
                                ContainerInfo {
                                    name: "app-server".to_string(),
                                    image: "node:18-alpine".to_string(),
                                },
                            ],
                        },
                    ],
                },
            ],
        },
        ClusterInfo {
            name: "development-cluster-02".to_string(),
            node_count: 2,
            pod_count: 12,
            nodes: vec![
                NodeInfo {
                    name: "dev-gpu-node-01".to_string(),
                    cluster_name: "development-cluster-02".to_string(),
                    os_image: "Ubuntu 22.04.3 LTS".to_string(),
                    kubelet_version: "v1.29.0".to_string(),
                    architecture: "amd64".to_string(),
                    capacity_cpu: "24".to_string(),
                    capacity_memory: "96Gi".to_string(),
                    gpu_model: "NVIDIA RTX 4090".to_string(),
                    gpu_count: "2".to_string(),
                    pod_count: 6,
                    container_count: 12,
                    labels: {
                        let mut labels = BTreeMap::new();
                        labels.insert("kubernetes.io/arch".to_string(), "amd64".to_string());
                        labels.insert("kubernetes.io/os".to_string(), "linux".to_string());
                        labels.insert("node.kubernetes.io/instance-type".to_string(), "gpu.2xlarge".to_string());
                        labels.insert("topology.kubernetes.io/zone".to_string(), "us-east-1a".to_string());
                        labels.insert("nvidia.com/gpu.present".to_string(), "true".to_string());
                        labels.insert("nvidia.com/gpu.count".to_string(), "2".to_string());
                        labels.insert("environment".to_string(), "development".to_string());
                        labels
                    },
                    mig_devices: HashMap::new(),
                    pods: vec![
                        PodInfo {
                            name: "dev-training-pod-a1b2c3".to_string(),
                            namespace: "dev-ml".to_string(),
                            node_name: "dev-gpu-node-01".to_string(),
                            cluster_name: "development-cluster-02".to_string(),
                            labels: {
                                let mut labels = BTreeMap::new();
                                labels.insert("app".to_string(), "model-training".to_string());
                                labels.insert("env".to_string(), "development".to_string());
                                labels.insert("framework".to_string(), "tensorflow".to_string());
                                labels
                            },
                            containers: vec![
                                ContainerInfo {
                                    name: "tensorflow-trainer".to_string(),
                                    image: "tensorflow/tensorflow:2.13.0-gpu".to_string(),
                                },
                            ],
                        },
                    ],
                },
                NodeInfo {
                    name: "dev-worker-node-01".to_string(),
                    cluster_name: "development-cluster-02".to_string(),
                    os_image: "Ubuntu 22.04.3 LTS".to_string(),
                    kubelet_version: "v1.29.0".to_string(),
                    architecture: "amd64".to_string(),
                    capacity_cpu: "8".to_string(),
                    capacity_memory: "32Gi".to_string(),
                    gpu_model: "N/A".to_string(),
                    gpu_count: "0".to_string(),
                    pod_count: 6,
                    container_count: 16,
                    labels: {
                        let mut labels = BTreeMap::new();
                        labels.insert("kubernetes.io/arch".to_string(), "amd64".to_string());
                        labels.insert("kubernetes.io/os".to_string(), "linux".to_string());
                        labels.insert("node.kubernetes.io/instance-type".to_string(), "c5.2xlarge".to_string());
                        labels.insert("topology.kubernetes.io/zone".to_string(), "us-east-1b".to_string());
                        labels.insert("environment".to_string(), "development".to_string());
                        labels
                    },
                    mig_devices: HashMap::new(),
                    pods: vec![
                        PodInfo {
                            name: "api-server-dev-x1y2z3".to_string(),
                            namespace: "api".to_string(),
                            node_name: "dev-worker-node-01".to_string(),
                            cluster_name: "development-cluster-02".to_string(),
                            labels: {
                                let mut labels = BTreeMap::new();
                                labels.insert("app".to_string(), "api-server".to_string());
                                labels.insert("env".to_string(), "development".to_string());
                                labels.insert("version".to_string(), "v0.8.1".to_string());
                                labels
                            },
                            containers: vec![
                                ContainerInfo {
                                    name: "api-container".to_string(),
                                    image: "python:3.11-slim".to_string(),
                                },
                            ],
                        },
                    ],
                },
            ],
        },
    ]
}