# OpenShell 전수조사 분석 정리 (한국어)

이 문서는 OpenShell 저장소를 전수조사하고 나눈 대화 내용을 정리한 기록이다.

## 저장소 정보

| 항목 | 내용 |
|---|---|
| 이 저장소 | https://github.com/bmshin94/OpenShell |
| 원본 (upstream) | https://github.com/NVIDIA/OpenShell |
| 커뮤니티 카탈로그 | https://github.com/NVIDIA/OpenShell-Community |
| 공식 문서 | https://docs.nvidia.com/openshell/latest |
| 로드맵 | https://github.com/orgs/NVIDIA/projects/233 |
| PyPI | https://pypi.org/project/openshell/ |
| 라이선스 | Apache 2.0 |
| 상태 | alpha (0.1.0 준비 중) |
| 작업 브랜치 | `claude/sharp-volta-z5mgww` |

이 저장소는 NVIDIA/OpenShell의 포크다. 원본과 다른 점은 `CLAUDE.md`에 페르소나 가이드를 추가한 커밋(`d67c3b7`, PR #1) 하나뿐이고, 나머지 히스토리는 전부 업스트림 그대로다.

## 1. OpenShell이 무엇인가

OpenShell은 자율 AI 에이전트를 위한 안전한 격리 실행 런타임이다. Claude Code, Codex, Cursor, OpenCode, GitHub Copilot CLI 같은 에이전트에게 터미널 권한을 주면 발생하는 문제 - 파일 삭제, 시크릿 유출, 무단 외부 통신, 프롬프트 인젝션 - 를 사람의 승인 팝업이 아니라 리눅스 커널 수준에서 강제 차단하는 방식으로 해결한다.

핵심 원칙은 다음 한 줄로 요약된다.

> 에이전트를 믿지 말고, 커널이 강제하게 하라.

### 사용하는 커널 기능

- **Landlock LSM** - 파일시스템 접근을 커널이 직접 제한
- **seccomp BPF** - 시스템 콜을 커널이 직접 필터링
- **no_new_privs / capability drop** - 권한 상승 경로 제거

## 2. 프로젝트 규모

| 지표 | 값 |
|---|---|
| Rust 파일 | 578개 |
| Rust 코드 | 약 511,000줄 |
| 크레이트 | 38개 |
| SDK | Python, TypeScript, Go, Rust (4종) |
| RFC 문서 | 13건 |
| protobuf 계약 | 10개 |
| 공개 에이전트 스킬 | 4개 (`skills/`) |
| 내부 컨트리뷰터 스킬 | 20개 (`.agents/skills/`) |
| 프로바이더 프로필 | 16종 (`providers/`) |
| 예제 | 16종 (`examples/`) |

## 3. 4계층 방어 구조

| 계층 | 막는 것 | 적용 시점 |
|---|---|---|
| Filesystem | 허용 경로 밖 읽기/쓰기 차단 | 샌드박스 생성 시 잠금 |
| Network | 미승인 아웃바운드 연결 차단 | 런타임 핫리로드 가능 |
| Process | 권한 상승 및 위험 syscall 차단 | 샌드박스 생성 시 잠금 |
| Providers | 엔드포인트 바인딩 크레덴셜만 주입 | 런타임 핫리로드 가능 |

## 4. 아키텍처

```
[CLI / SDK / TUI]
       | gRPC
   [Gateway]            컨트롤 플레인, 인증 경계, 영속 상태
       | 컴퓨트 드라이버 (Docker / Podman / K8s / MicroVM / MXC)
 +------ Sandbox 워크로드 ------+
 |  [Supervisor]               정책 집행, L7 프록시, 크레덴셜 (워크로드 바깥)
 |        | mTLS over UDS/vsock
 |  [openshell-sandbox]        seccomp 리스너, Landlock 베이스라인
 |        | spawn
 |  [Agent child]              capability 0, no_new_privs, Landlock, seccomp
 +------------------------------+
```

### 컴포넌트 역할

| 컴포넌트 | 역할 |
|---|---|
| Gateway | 샌드박스 생명주기 관리, 인증 경계, 정책/설정 전달 |
| Supervisor | 워크로드 외부에서 모든 egress를 가로채 정책 평가, 크레덴셜 주입 |
| Sandbox | 워크로드 내부 감시자, 자식 프로세스 소유, 보호 채널 중재 |
| Agent child | 실제 에이전트 프로세스. 권한 없음 |

## 5. 핵심 기능 3가지

### L7 단위 네트워크 정책

호스트 허용/차단이 아니라 HTTP 메서드와 경로까지 검사하고, 어떤 바이너리가 요청했는지도 procfs로 검증한다.

```yaml
version: 1
filesystem_policy:
  include_workdir: true
  read_only: [/usr, /lib, /proc, /dev/urandom, /app, /etc, /var/log]
  read_write: [/sandbox, /tmp, /dev/null]
landlock:
  compatibility: best_effort
network_policies:
  github_api:
    name: github-api-readonly
    endpoints:
      - host: api.github.com
        port: 443
        protocol: rest
        enforcement: enforce
        access: read-only
    binaries:
      - { path: /usr/bin/curl }
```

동작 결과는 다음과 같다.

```
curl https://api.github.com/zen           -> 200 OK
curl -X POST https://api.github.com/...   -> {"error":"policy_denied"}
```

### 엔드포인트 바인딩 크레덴셜 주입

API 키가 샌드박스 파일시스템이나 환경변수에 노출되지 않는다. Supervisor가 정책을 통과한 요청에 한해, 승인된 호스트로 나갈 때만 헤더에 주입한다. 에이전트가 base URL을 바꿔도 크레덴셜은 따라가지 않는다.

```yaml
id: anthropic
credentials:
  - name: api_key
    env_vars: [ANTHROPIC_API_KEY]
    auth_style: header
    header_name: x-api-key
endpoints:
  - host: api.anthropic.com
    port: 443
    access: read-write
    enforcement: enforce
binaries: [/usr/bin/curl, /usr/local/bin/curl]
```

### OCSF 구조화 보안 로그

모든 허용/차단/우회 탐지/SSH 인증/프로세스 생명주기 이벤트를 OCSF v1.8.0 표준으로 발행해 Splunk, Datadog, Elastic 등 SIEM에 바로 연동할 수 있다.

## 6. 디렉터리 구조

| 경로 | 내용 |
|---|---|
| `crates/` | 38개 Rust 크레이트. 프로젝트 핵심 |
| `proto/` | gRPC 계약 10개. 드라이버/미들웨어 확장 지점 |
| `skills/` | 공개 에이전트 스킬 4개 |
| `.agents/skills/` | 내부 컨트리뷰터 워크플로 20개 |
| `providers/` | 프로바이더 프로필 예시 16종 |
| `examples/` | 실전 예제 16종 |
| `rfc/` | 설계 제안서 13건 |
| `docs/`, `fern/` | 퍼블리시 문서 및 사이트 설정 |
| `architecture/` | 서브시스템 설계 문서 9개 |
| `deploy/` | deb, rpm, helm, kube, docker, man 패키징 |
| `e2e/` | E2E 테스트 (docker, k8s, gpu, MCP 적합성 포함) |
| `sdk/`, `python/` | Go, TypeScript, Python SDK |
| `install.sh` | 플랫폼 자동 감지 설치 스크립트 |
| `mise.toml`, `tasks/` | 개발 툴체인 및 태스크 정의 |

### 주요 크레이트

| 크레이트 | 역할 |
|---|---|
| `openshell-cli` | `openshell` 명령어 |
| `openshell-server` | 게이트웨이 컨트롤 플레인 |
| `openshell-sandbox` | 워크로드 내부 seccomp/Landlock 적용 |
| `openshell-supervisor` | 정책 집행, 크레덴셜, 업스트림 네트워킹 |
| `openshell-policy`, `openshell-policy-schema` | 정책 엔진 및 YAML 스키마 |
| `openshell-prover` | Z3 SMT 솔버 기반 정책 수학적 검증 |
| `openshell-driver-*` | docker, podman, kubernetes, vm, mxc 컴퓨트 백엔드 |
| `openshell-ocsf` | OCSF 이벤트 빌더 및 포매터 |
| `openshell-tui` | k9s 스타일 실시간 대시보드 |

## 7. 설치 및 사용법

### 사전 요구사항

- Linux (권장), macOS Apple Silicon, 또는 Windows WSL 2 (실험적)
- Docker, Podman, 또는 호스트 가상화 (MicroVM)
- Landlock 지원 커널

### 설치

```shell
curl -LsSf https://raw.githubusercontent.com/NVIDIA/OpenShell/main/install.sh | sh
```

개발 빌드는 `OPENSHELL_VERSION=dev`, 프리릴리즈는 `OPENSHELL_VERSION=pre`를 지정한다. 프리릴리즈는 GitHub Actions 아티팩트라 `gh auth login`이 필요하다.

### 기본 사용

```shell
openshell sandbox create -- claude
openshell sandbox connect <name>
openshell policy set <name> --policy policy.yaml --wait
openshell logs <name> --tail
openshell term
```

### Kubernetes 배포

```shell
helm install openshell oci://ghcr.io/nvidia/openshell/helm-chart \
  --set supervisor.sandboxRuntime.networkPolicyEnforced=true
```

### 소스 개발

```shell
mise run pre-commit
mise run test
mise run e2e
mise run ci
```

## 8. 플러그인인가, 스킬인가, MCP인가

셋 다 아니다. OpenShell은 그보다 한 층 아래에 있는 **런타임 인프라**다.

- 플러그인/스킬/MCP는 에이전트가 **무엇을 할 수 있는지**를 늘린다.
- OpenShell은 에이전트가 **무엇을 절대 할 수 없는지**를 커널로 강제한다.

혼동하기 쉬운 지점은 다음과 같다.

- OpenShell은 Agent Skills를 **제공한다**. `npx skills add NVIDIA/OpenShell`로 `openshell-cli`, `debug-openshell-cluster`, `debug-inference`, `generate-sandbox-policy` 4종을 설치할 수 있다. OpenShell을 조종하는 스킬이지, OpenShell 자체가 스킬은 아니다.
- MCP 서버가 아니다. 대신 `e2e/mcp-conformance/`에서 MCP 트래픽이 샌드박스 프록시를 통과하는지 적합성 테스트를 수행한다. MCP는 보호 대상이다.
- Claude Code 플러그인이 아니다. 반대로 Claude Code가 OpenShell 안에서 실행된다.

## 9. API 토큰 필요 여부

| 레이어 | 토큰 필요 | 설명 |
|---|---|---|
| OpenShell 설치 및 실행 | 불필요 | 프리릴리즈 설치만 `gh auth login` 필요 |
| CLI 게이트웨이 인증 | 필요하나 자동 | 로컬은 mTLS 자동 발급. 원격은 Bearer, 팀은 OIDC |
| 에이전트가 쓸 API 키 | 필요 | 단 Provider로 등록하며 샌드박스에 노출되지 않음 |

크레덴셜은 `~/.config/openshell/gateways/<name>/`에 저장되고, 프로바이더 크레덴셜은 게이트웨이 크레덴셜 스토어(Vault, Kubernetes Secret, DB 드라이버)에 보관된다.

## 10. GitHub에서 유명한 이유

- NVIDIA 공식 프로젝트이며 GTC 2026 발표, RSAC 2026 피처를 거쳤다.
- 2025-2026년 AI 에이전트 확산과 동시에 터진 보안 문제에 정확히 대응하는 타이밍이었다.
- 승인 팝업이나 정규식 필터가 아니라 커널 primitive로 강제하는 근본적 접근을 택했다.
- Rust 51만 줄, RFC 프로세스, DCO, 거버넌스, Z3 기반 정책 증명기 등 구현 품질이 높다.
- 자신들이 만드는 도구로 자신들의 개발을 수행하는 agent-first 서사가 커뮤니티에서 공감을 얻었다.
- 주요 에이전트와 컴퓨트 플랫폼을 폭넓게 지원하고 Apache 2.0이라 기업 도입 장벽이 낮다.

참고 자료.

- https://github.com/NVIDIA/openshell
- https://docs.nvidia.com/openshell/about/overview
- https://forkast.news/nvidia-openshell-ships-policy-based-sandboxing-as-a-runtime-enforcement-layer-for-autonomous-agents/
- https://medium.com/@priyanchew/openshell-why-nvidia-is-building-linux-for-the-age-of-ai-agents-29c4939ab47e
- https://davidkaya.com/blog/nvidia-openshell-the-sandbox-ai-agents-have-been-missing/

## 11. 로컬 에이전트 구축에 도움이 되는가

| 목적 | 적합도 | 근거 |
|---|---|---|
| 로컬에서 코딩 에이전트를 풀권한으로 안전하게 실행 | 매우 높음 | 이 도구의 정확한 목적 |
| 에이전트 다중 병렬 실행 | 매우 높음 | 샌드박스별 완전 격리 |
| 로컬 LLM(Ollama, vLLM, LM Studio, NIM) 연동 | 매우 높음 | `examples/local-inference`, `debug-inference` 스킬 |
| 신뢰할 수 없는 MCP 서버 테스트 | 매우 높음 | MCP 적합성 테스트 내장 |
| 민감 데이터 취급 에이전트 | 매우 높음 | 정책 잠금 + OCSF 감사 로그 |
| 에이전트 프레임워크 자체 개발 | 보통 | 실행 계층만 담당. LangGraph 대체 아님 |
| 단순 챗봇 | 낮음 | 과도한 구성 |

한계는 다음과 같다. alpha 단계라 브레이킹 체인지 가능성이 있고, Linux 중심이며 Windows 네이티브는 RFC 0013 진행 중이다. 정책 YAML과 게이트웨이 개념에 대한 학습 곡선이 있고, 게이트웨이 데몬과 컨테이너 리소스 오버헤드가 존재한다.

## 12. React 또는 PHP로 만들 수 있는가

### 코어 재구현은 불가능하다

Landlock, seccomp BPF, seccomp user notify, procfs 바이너리 신원 확인, capability 제어, vsock mTLS는 모두 커널 시스템 콜 직접 제어가 필요하다. Node.js나 PHP 런타임에서는 접근할 수 없고, 모든 패킷이 프록시를 통과하므로 GC 있는 런타임은 성능 병목이 된다. Rust를 선택한 이유는 GC 없음, 메모리 안전, C 수준 syscall 접근, 단일 바이너리 배포다.

### 위에 올리는 레이어는 충분히 가능하다

OpenShell은 gRPC API와 4종 SDK를 제공하므로 상위 애플리케이션은 자유롭게 구성할 수 있다.

React로 만들 수 있는 것.

- 웹 대시보드 (TUI의 웹 버전). `@nvidia/openshell-sdk`는 Connect 클라이언트라 브라우저에서 직접 호출 가능
- 정책 비주얼 에디터 (React Flow + Monaco Editor)
- OCSF 로그 기반 실시간 보안 모니터링 화면
- xterm.js 기반 멀티 에이전트 오케스트레이션 UI

PHP로 만들 수 있는 것. 공식 SDK는 없으나 `proto/`에서 gRPC PHP 클라이언트를 생성하거나, CLI를 JSON 출력으로 래핑하거나, Node/Python 사이드카를 두는 방식이 가능하다. Laravel 백오피스, 승인 워크플로, 멀티테넌트 포털 등이 적합하다.

권장 구성은 다음과 같다.

```
[React + TypeScript]   대시보드, 정책 에디터, 모니터링
        | REST / WebSocket
[PHP Laravel or Node]  인증, 과금, 팀 관리, 감사 로그 저장
        | gRPC (@nvidia/openshell-sdk)
[OpenShell Gateway]    그대로 사용
        |
[Sandboxes]
```

## 13. 수익화 아이디어

시장 구조는 단순하다. 엔진은 Apache 2.0 무료 오픈소스이고, 기업과 개발자에게는 예산은 있으나 시간과 전문성이 없다. 그 사이의 공백 - 복잡도, UI 부재, 운영 부담, 컴플라이언스, 언어 장벽, 통합 - 이 수익 지점이다.

### 후보 정리

| 번호 | 아이디어 | 난이도 | 수익 잠재력 |
|---|---|---|---|
| 1 | Policy Studio - 정책 YAML 웹 에디터 SaaS | 중 | 높음 |
| 2 | Managed OpenShell Cloud - 호스팅 게이트웨이 | 상 | 매우 높음 |
| 3 | AgentAudit - OCSF 기반 컴플라이언스 대시보드 | 중상 | 매우 높음 |
| 4 | 한국어 교육 콘텐츠 및 커뮤니티 | 하 | 중 |
| 5 | 기업 도입 컨설팅 및 SI | 중 | 매우 높음 |
| 6 | 커스텀 샌드박스 이미지 마켓플레이스 | 중 | 중 |
| 7 | 정책 팩 구독 서비스 | 중 | 중 |
| 8 | 커스텀 드라이버 및 미들웨어 개발 | 상 | 중 |

### Policy Studio

정책 작성이 OpenShell의 최대 진입 장벽이고, 웹에서 팀이 함께 편집하는 도구는 아직 없다. 비주얼 에디터와 실시간 YAML 프리뷰, 템플릿 갤러리를 무료로 제공하고, 자연어 기반 정책 생성과 `openshell-prover` 연동 안전성 증명, 팀 협업과 승인 워크플로, 조직 정책 상속과 SSO를 유료화한다. 가격은 Pro 19달러/월, Team 99달러/월(5석), Enterprise 1,000달러/월 이상을 기준으로 한다. 정책은 텍스트이므로 서버에서 OpenShell 바이너리를 돌릴 필요조차 없어 MVP를 2-4주에 만들 수 있다.

### Managed OpenShell Cloud

게이트웨이 운영 부담을 대신 지는 호스팅 서비스다. 웹에서 템플릿과 레포를 고르면 30초 안에 브라우저 터미널이 열리는 경험을 판다. 브라우저 터미널, GitHub App 연동, 즉시 스케일, 실시간 보안 피드, 유휴 하이버네이션, 스케줄 실행이 차별점이다. Hobby 무료, Pro 29달러/월, Team 199달러/월, 초과분 시간당 0.15달러 구조가 적합하다. 인프라 비용이 마진을 결정하고, 채굴 등 어뷰징 방어가 필수이며, alpha 위에 올리는 만큼 브레이킹 체인지 대응 부담이 있다.

### AgentAudit

OpenShell은 이미 OCSF 표준 로그를 발행하지만 그것을 읽고 해석하는 제품이 없다. 기업이 실제로 예산을 쓰는 지점은 보안 자체가 아니라 감사 통과다. 한국은 ISMS-P와 전자금융감독규정 때문에 AI 행위 증빙이 실질적 요구사항이 되고 있다. 실시간 이벤트 스트림, 이상 탐지, 원클릭 감사 리포트 PDF, 데이터 흐름 맵, 정책 준수 스코어, Slack/PagerDuty 알림, SIEM 익스포트를 제공한다. Starter 99달러/월, Business 499달러/월, Enterprise 2,000달러/월 이상이 기준이다. 수집기는 Go 또는 Node, 저장은 ClickHouse 또는 TimescaleDB, 화면은 React로 구성한다.

### 한국어 교육 콘텐츠

한국어 자료가 사실상 없다. 블로그와 영상으로 시작해 온라인 강의, 전자책, 기업 사내 교육, 유료 커뮤니티로 확장하는 경로가 가능하다. 초기 수익은 크지 않으나 신뢰 자산이 쌓이고 컨설팅 리드가 유입된다.

### 기업 도입 컨설팅

가장 빠른 현금화 경로다. 보안 진단 리포트, PoC 구축, 프로덕션 구축, 유지보수로 패키지를 나눈다. 금융, 헬스케어, 게임, 제조 대기업, 시리즈 B 이상 스타트업이 주요 타겟이다. 초기 자본이 들지 않고 고객 요구사항을 학습해 이후 SaaS 기획에 그대로 활용할 수 있다.

### 실행 순서 제안

1. 0-1개월. 직접 구축하고 한국어 콘텐츠를 발행해 신뢰를 쌓는다.
2. 1-3개월. Policy Studio MVP를 만들어 공개 채널에 런칭한다.
3. 2-4개월. 컨설팅을 병행해 현금흐름을 확보하고 실제 요구사항을 수집한다.
4. 4-8개월. 수집한 요구사항으로 AgentAudit을 개발한다.
5. 8개월 이후. 자본과 팀을 확보한 뒤 Managed Cloud에 도전한다.

### 라이선스 확인 사항

| 항목 | 가능 여부 |
|---|---|
| 상업적 이용 | 가능 |
| 수정 후 비공개 판매 | 가능 |
| 저작권 및 NOTICE 표기 | 필수 |
| NVIDIA 상표 사용 | 불가. "OpenShell 기반"으로 표현 |
| 특허 | Apache 2.0 특허 라이선스 포함 |

## 14. 핵심 요약

- OpenShell은 AI 에이전트를 커널 수준으로 격리 실행하는 NVIDIA의 오픈소스 런타임이다.
- 플러그인도 스킬도 MCP도 아니며, 그것들보다 한 층 아래의 실행 인프라다.
- 설치 자체에 토큰은 필요 없고, 에이전트용 API 키는 Provider로 등록해 샌드박스에 노출시키지 않는다.
- 코어는 Rust와 커널 기능에 의존하므로 React나 PHP로 재구현할 수 없지만, gRPC API 위의 UI와 SaaS 레이어는 자유롭게 만들 수 있다.
- 수익화는 엔진이 아니라 경험을 파는 방향이 맞다. 컨설팅으로 현금흐름을 만들고 그 학습으로 SaaS를 만드는 순서를 권한다.
