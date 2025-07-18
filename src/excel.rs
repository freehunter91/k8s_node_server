use crate::models::NodeInfo;
use rust_xlsxwriter::*;

pub fn generate_node_excel(node_info: &NodeInfo) -> Result<Vec<u8>, XlsxError> {
    let mut workbook = Workbook::new();
    
    // 헤더 스타일
    let header_format = Format::new()
        .set_bold()
        .set_background_color(Color::RGB(0xD3D3D3))
        .set_border(FormatBorder::Thin);
    
    let data_format = Format::new()
        .set_border(FormatBorder::Thin);
    
    // 노드 기본 정보 시트
    let worksheet = workbook.add_worksheet().set_name("Node Info")?;
    
    // 노드 기본 정보
    worksheet.write_with_format(0, 0, "항목", &header_format)?;
    worksheet.write_with_format(0, 1, "값", &header_format)?;
    
    let mut row = 1;
    let pod_count_str = node_info.pod_count.to_string();
    let container_count_str = node_info.container_count.to_string();
    
    let node_data = vec![
        ("노드 이름", node_info.name.as_str()),
        ("클러스터", node_info.cluster_name.as_str()),
        ("OS 이미지", node_info.os_image.as_str()),
        ("Kubelet 버전", node_info.kubelet_version.as_str()),
        ("아키텍처", node_info.architecture.as_str()),
        ("CPU 용량", node_info.capacity_cpu.as_str()),
        ("메모리 용량", node_info.capacity_memory.as_str()),
        ("GPU 모델", node_info.gpu_model.as_str()),
        ("GPU 개수", node_info.gpu_count.as_str()),
        ("Pod 수", pod_count_str.as_str()),
        ("Container 수", container_count_str.as_str()),
    ];
    
    for (key, value) in node_data {
        worksheet.write_with_format(row, 0, key, &data_format)?;
        worksheet.write_with_format(row, 1, value, &data_format)?;
        row += 1;
    }
    
    // 라벨 정보
    if !node_info.labels.is_empty() {
        row += 1;
        worksheet.write_with_format(row, 0, "라벨", &header_format)?;
        worksheet.write_with_format(row, 1, "값", &header_format)?;
        row += 1;
        
        for (key, value) in &node_info.labels {
            worksheet.write_with_format(row, 0, key, &data_format)?;
            worksheet.write_with_format(row, 1, value, &data_format)?;
            row += 1;
        }
    }
    
    // MIG 디바이스 정보
    if !node_info.mig_devices.is_empty() {
        row += 1;
        worksheet.write_with_format(row, 0, "MIG 디바이스", &header_format)?;
        worksheet.write_with_format(row, 1, "개수", &header_format)?;
        row += 1;
        
        for (key, value) in &node_info.mig_devices {
            worksheet.write_with_format(row, 0, key, &data_format)?;
            worksheet.write_with_format(row, 1, value, &data_format)?;
            row += 1;
        }
    }
    
    // 컬럼 너비 조정
    worksheet.set_column_width(0, 20.0)?;
    worksheet.set_column_width(1, 30.0)?;
    
    // Pod 정보 시트
    let pod_worksheet = workbook.add_worksheet().set_name("Pods")?;
    
    // Pod 헤더
    let pod_headers = vec![
        "Pod 이름", "네임스페이스", "Container 수", "라벨"
    ];
    
    for (col, header) in pod_headers.iter().enumerate() {
        pod_worksheet.write_with_format(0, col as u16, *header, &header_format)?;
    }
    
    // Pod 데이터
    for (row, pod) in node_info.pods.iter().enumerate() {
        let row = row + 1;
        pod_worksheet.write_with_format(row as u32, 0, &pod.name, &data_format)?;
        pod_worksheet.write_with_format(row as u32, 1, &pod.namespace, &data_format)?;
        pod_worksheet.write_with_format(row as u32, 2, pod.containers.len() as u32, &data_format)?;
        
        let labels_str = pod.labels.iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join(", ");
        pod_worksheet.write_with_format(row as u32, 3, &labels_str, &data_format)?;
    }
    
    pod_worksheet.set_column_width(0, 30.0)?;
    pod_worksheet.set_column_width(1, 20.0)?;
    pod_worksheet.set_column_width(2, 15.0)?;
    pod_worksheet.set_column_width(3, 50.0)?;
    
    // Container 정보 시트
    let container_worksheet = workbook.add_worksheet().set_name("Containers")?;
    
    // Container 헤더
    let container_headers = vec![
        "Pod 이름", "네임스페이스", "Container 이름", "이미지"
    ];
    
    for (col, header) in container_headers.iter().enumerate() {
        container_worksheet.write_with_format(0, col as u16, *header, &header_format)?;
    }
    
    // Container 데이터
    let mut container_row = 1;
    for pod in &node_info.pods {
        for container in &pod.containers {
            container_worksheet.write_with_format(container_row, 0, &pod.name, &data_format)?;
            container_worksheet.write_with_format(container_row, 1, &pod.namespace, &data_format)?;
            container_worksheet.write_with_format(container_row, 2, &container.name, &data_format)?;
            container_worksheet.write_with_format(container_row, 3, &container.image, &data_format)?;
            container_row += 1;
        }
    }
    
    container_worksheet.set_column_width(0, 30.0)?;
    container_worksheet.set_column_width(1, 20.0)?;
    container_worksheet.set_column_width(2, 25.0)?;
    container_worksheet.set_column_width(3, 60.0)?;
    
    workbook.save_to_buffer()
}