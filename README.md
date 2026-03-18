# jaturi

`jaturi`는 OpenAI를 사용해 단어를 자동 생성하고, 터미널에서 학습과 퀴즈를 진행하는 언어 학습 앱입니다.

## 이 서비스가 하는 일

- API Key를 입력하면 AI가 학습 주제와 단어를 자동 생성
- 최근 10개 주제와 겹치지 않는 새 주제를 우선 생성
- 현재 언어 실력 + 앱 레벨을 반영해 단어 난이도를 조정
- 단어 학습(뜻 확인) 후 바로 퀴즈로 복습
- 언어별 진행도/점수/XP를 로컬에 저장
- 주제 생성 시 단어 수는 항상 `10`개
- 퀴즈 점수 90점 이상 시 실력 보상(+1), 100점 달성 시 추가 +1 (주제별 1회)
- 학습 XP는 같은 주제 첫 완료 `+10`, 재학습 완료 `+5`

## 다운로드 및 전역 설치 (운영체제별)

릴리스 페이지: `https://github.com/ium-team/jaturi/releases/latest`

압축을 풀고 설치 스크립트를 1번 실행하면, PATH에 등록되어 어느 경로에서든 `jaturi`를 실행할 수 있습니다.

### Linux (x86_64)

```bash
VERSION="v0.1.5"
curl -LO "https://github.com/ium-team/jaturi/releases/download/${VERSION}/jaturi-${VERSION}-x86_64-unknown-linux-musl.tar.gz"
tar -xzf "jaturi-${VERSION}-x86_64-unknown-linux-musl.tar.gz"
chmod +x install.sh
./install.sh
export PATH="$HOME/.local/bin:$PATH"
jaturi
```

### macOS (Apple Silicon)

```bash
VERSION="v0.1.5"
curl -LO "https://github.com/ium-team/jaturi/releases/download/${VERSION}/jaturi-${VERSION}-aarch64-apple-darwin.tar.gz"
tar -xzf "jaturi-${VERSION}-aarch64-apple-darwin.tar.gz"
chmod +x install.sh
./install.sh
export PATH="$HOME/.local/bin:$PATH"
jaturi
```

### Windows (x86_64, PowerShell)

```powershell
$Version = "v0.1.5"
Invoke-WebRequest -Uri "https://github.com/ium-team/jaturi/releases/download/$Version/jaturi-$Version-x86_64-pc-windows-msvc.zip" -OutFile "jaturi-$Version-x86_64-pc-windows-msvc.zip"
Expand-Archive -Path "jaturi-$Version-x86_64-pc-windows-msvc.zip" -DestinationPath "." -Force
.\install.ps1
jaturi
```

참고:
- 터미널 기반 앱(TUI)이므로 반드시 터미널에서 실행하세요.
- Linux/macOS는 기본적으로 `~/.local/bin`, Windows는 `%LOCALAPPDATA%\Programs\jaturi\bin`에 설치됩니다.
- PATH 반영을 위해 터미널을 새로 열어야 할 수 있습니다.
- macOS/Windows는 서명되지 않은 앱 경고가 보일 수 있습니다.

## 사용 방법

### 1) 처음 실행

- `API Key`, `이름`, `학습 언어`를 입력하고 `S`로 저장
- 저장 후 메인 화면으로 이동

### 2) 주제 생성

- 메인에서 `N` 눌러 주제 생성 화면 이동
- `Enter`를 누르면 AI가 주제 + 단어 10개 자동 생성
- 최근 10개 주제와 겹치는 주제는 피하고, 현재 학습 수준에 맞는 단어를 우선 생성

### 3) 학습

- 단어/뜻을 보고 `Enter`로 다음 단어 이동
- 필요하면 `Q`로 바로 퀴즈 시작

### 4) 퀴즈

- 객관식: `Up/Down` 선택, `Enter` 제출
- 주관식(스펠링): 입력 후 `Enter` 제출
- 점수 90점 이상이면 해당 주제 실력 `+1`
- 같은 주제에서 100점을 추가 달성하면 실력 `+1` 추가 (총 +2)

### XP 규칙

- 학습 완료 XP: 해당 주제 첫 완료 `+10`, 재학습 완료 `+5`
- 퀴즈 완료 XP: 항상 `+5`

### 5) 결과/복습

- 결과 화면에서 `M`(메인), `S`(같은 주제 학습), `Q`(같은 주제 퀴즈)

## 키 조작 요약

- 공통: `Esc` 종료
- 설정 화면: `Up/Down`, `Tab` 이동, `Enter` 편집 시작/종료, `S` 저장
- 메인 화면: `N` 새 주제, `S`/`Enter` 학습, `Q` 퀴즈, `K` 설정
- 주제 생성: `Enter` 생성 시작, `M` 메인 복귀
- 학습: `Enter` 다음 단어, `Q` 퀴즈
- 퀴즈: 객관식 `Up/Down + Enter`, 주관식 `입력 + Enter`
- 결과: `M` 메인, `S` 학습, `Q` 퀴즈
- 에러: `R` 복구/재시도, `Q` 종료

## 데이터 저장

- 학습 데이터는 OS의 사용자 데이터 디렉터리에 저장됩니다.
- 기존 `vocab_tui` 데이터를 사용 중이었다면 첫 실행 시 `jaturi` 경로로 자동 마이그레이션됩니다.
