# --- 1단계: 빌드 환경 (Builder) ---
# Rust 코드를 컴파일하기 위한 공식 Rust 이미지를 사용합니다.
# 'slim' 버전을 사용하여 빌드 환경의 크기를 줄입니다.
FROM docker.io/rust:bookworm as builder

# 작업 디렉터리 설정
WORKDIR /usr/src/app

# 의존성 파일만 먼저 복사하여 Docker의 레이어 캐싱을 활용합니다.
# Cargo.toml이 변경되지 않으면 이 단계는 다시 실행되지 않아 빌드 속도가 빨라집니다.
COPY Cargo.toml Cargo.lock ./

# 의존성만 먼저 빌드합니다.
# 빈 lib.rs를 만들어 의존성 다운로드 및 컴파일을 강제합니다.
RUN mkdir src && echo "fn main() {}" > src/lib.rs && cargo build --release --target=x86_64-unknown-linux-gnu
# 전체 소스 코드를 복사합니다.
COPY src ./src

# 소스 코드가 변경되었으므로 다시 빌드하여 최신 실행 파일을 만듭니다.
# 'touch' 명령어로 main.rs의 수정 시간을 변경하여 cargo가 다시 컴파일하도록 합니다.
RUN touch src/main.rs && cargo build --release --target=x86_64-unknown-linux-gnu

# --- 2단계: 실행 환경 (Final Image) ---
# 실제 운영 환경은 작고 가벼운 데비안 이미지를 기반으로 합니다.
FROM debian:bookworm-slim

# HTTPS 통신 등에 필요한 SSL 인증서 관련 패키지를 설치합니다.
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

# 빌드 단계에서 생성된 최적화된 실행 파일을 복사합니다.
COPY --from=builder /usr/src/app/target/x86_64-unknown-linux-gnu/release/k8s_node_server /usr/local/bin/

# 프론트엔드 파일(index.html)이 담긴 static 폴더를 복사합니다.
COPY static /static

# 작업 디렉터리 설정
WORKDIR /

# 서버가 8080 포트를 외부에 노출함을 명시합니다.
EXPOSE 8080

# 컨테이너가 시작될 때 실행할 기본 명령어를 정의합니다.
CMD ["/usr/local/bin/k8s_node_server"]
