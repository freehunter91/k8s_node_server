# K8s 통합 대시보드

Rust와 Vue.js로 제작된 경량 쿠버네티스(Kubernetes) 리소스 모니터링 대시보드입니다. 이 애플리케이션은 로컬 머신에 설정된 `~/.kube/config` 파일을 읽어 여러 클러스터의 노드 및 파드 정보를 웹 인터페이스를 통해 시각적으로 보여주고, 강력한 검색 기능을 제공합니다.

## ✨ 주요 기능

* **통합 뷰**: 여러 쿠버네티스 클러스터의 정보를 하나의 대시보드에서 확인
* **상세 정보**: 클러스터, 노드, 파드별 상세 정보 및 라벨, 컨테이너 정보 조회
* **빠른 검색**: 노드 또는 파드 이름으로 전체 클러스터에서 리소스를 신속하게 검색
* **경량 및 고성능**: Rust 기반의 고성능 백엔드로 최소한의 리소스를 사용
* **단일 파일 배포**: 백엔드 서버가 직접 프론트엔드(HTML)를 서빙하여 실행 파일 하나로 간편하게 배포 가능

## 🛠️ 기술 스택

### 백엔드 (Backend)

* **언어**: Rust
* **웹 프레임워크**: Actix-web
* **쿠버네티스 클라이언트**: kube-rs
* **로깅**: env_logger, log

### 프론트엔드 (Frontend)

* **라이브러리**: Vue.js 3 (CDN 방식)
* **스타일링**: Tailwind CSS (CDN 방식)

## ⚙️ 로직 상세 설명

(이전 내용과 동일)

## 🚀 실행 및 배포

이 애플리케이션은 로컬 환경에서 직접 실행하거나, Docker 컨테이너로 만들어 쿠버네티스에 배포할 수 있습니다.

### 방법 1: 로컬 환경에서 직접 실행

로컬 개발 및 테스트에 적합한 방법입니다.

1.  **빌드:**
    프로젝트 최상위 디렉터리에서 릴리즈 모드로 애플리케이션을 빌드합니다.
    ```bash
    cargo build --release
    ```

2.  **실행:**
    빌드가 완료되면 아래 명령어로 서버를 시작합니다.
    ```bash
    RUST_LOG=info ./target/release/k8s_node_server
    ```

3.  **접속:**
    웹 브라우저에서 `http://127.0.0.1:8080`으로 접속합니다.

    > **중요:** 이 방법은 실행하는 머신에 `~/.kube/config` 파일이 올바르게 설정되어 있어야 쿠버네티스 클러스터 정보를 읽어올 수 있습니다.

### 방법 2: 쿠버네티스에 배포 (Docker 사용)

실제 운영 환경에 배포하기 위한 권장 방법입니다.

#### 1단계: Dockerfile 작성

프로젝트 최상위 디렉터리에 아래 내용으로 `Dockerfile`을 생성합니다.

```dockerfile
# --- 1단계: 빌드 환경 (Builder) ---
FROM rust:1-slim as builder
WORKDIR /usr/src/app
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/lib.rs && cargo build --release
COPY src ./src
RUN touch src/main.rs && cargo build --release

# --- 2단계: 실행 환경 (Final Image) ---
FROM debian:bullseye-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /usr/src/app/target/release/k8s_node_server /usr/local/bin/
COPY static /static
WORKDIR /
EXPOSE 8080
CMD ["/usr/local/bin/k8s_node_server"]
```

#### 2단계: Docker 이미지 빌드

터미널에서 위 `Dockerfile`이 있는 경로에서 아래 명령어를 실행하여 Docker 이미지를 만듭니다.
```bash
docker build -t k8s-dashboard:latest .
```

#### 3단계: 컨테이너 레지스트리에 푸시

빌드된 이미지를 쿠버네티스 클러스터가 접근할 수 있는 컨테이너 레지스트리(예: Docker Hub, GCR, Harbor 등)에 푸시해야 합니다.

먼저, 레지스트리 주소에 맞게 이미지에 태그를 지정합니다.
```bash
# 예시: docker tag k8s-dashboard:latest your-registry/your-repo/k8s-dashboard:v1.0
docker tag k8s-dashboard:latest gwon/k8s-dashboard:v1.0
```

그 다음, 레지스트리에 이미지를 푸시합니다.
```bash
# 예시: docker push your-registry/your-repo/k8s-dashboard:v1.0
docker push gwon/k8s-dashboard:v1.0
```

#### 4단계: `k8s-dashboard-deployment.yaml` 파일 수정

배포 YAML 파일의 `image` 필드를 방금 푸시한 이미지의 전체 주소로 수정해야 합니다. (이 파일은 '전체 소스 코드' 섹션에 포함되어 있습니다.)

```yaml
# k8s-dashboard-deployment.yaml 파일의 일부
...
      containers:
      - name: k8s-dashboard-container
        # 중요: 이 부분을 3단계에서 푸시한 이미지 주소로 수정합니다.
        image: gwon/k8s-dashboard:v1.0 
...
```

#### 5단계: 쿠버네티스에 배포

수정된 YAML 파일을 `kubectl`을 사용하여 클러스터에 적용합니다. 이 명령은 필요한 모든 리소스(ServiceAccount, Deployment, Service 등)를 생성합니다.
```bash
kubectl apply -f k8s-dashboard-deployment.yaml
```

#### 6단계: 서비스 접속 확인

배포가 완료되면, 아래 명령어로 외부에서 접속할 수 있는 포트(NodePort)를 확인합니다.
```bash
kubectl get svc k8s-dashboard-service
```
명령어 결과에서 `PORT(S)` 항목을 확인합니다. 예를 들어 `80:30080/TCP`와 같이 표시된다면, `30080`이 외부 접속 포트입니다.

이제 웹 브라우저에서 `http://<클러스터_노드_중_하나의_IP>:<NodePort>` 주소로 접속하여 대시보드가 보이는지 확인합니다.

