# jaturi (Rust)

OpenAI `gpt-4o-mini`를 사용해 영어 단어를 생성하고, TUI에서 학습/퀴즈를 진행하는 프로그램입니다.

## 기능

- 사용자 입력 API Key로 OpenAI 호출
- AI가 학습 주제(Topic) 자동 생성
- 생성 단어 수는 항상 `10`개로 고정
- 학습 모드: 단어/뜻/예문 확인
- 퀴즈 모드: 객관식 의미 맞추기

## 실행

```bash
cargo run
```

## 배포(다른 사람에게 공유)

이 프로젝트는 GitHub Actions 릴리스 워크플로우가 설정되어 있어, 태그를 푸시하면 OS별 실행 파일이 자동으로 올라갑니다.

1. 버전 태그 생성

```bash
git tag v0.1.1
git push origin v0.1.1
```

`git tag`만 로컬에서 만들면 실행되지 않고, 반드시 `git push origin v0.1.1`까지 해야 워크플로우가 실행됩니다.

2. GitHub `Releases`에서 생성된 파일 다운로드

- Linux(정적 링크): `jaturi-v0.1.1-x86_64-unknown-linux-musl.tar.gz`
- macOS(Apple Silicon): `jaturi-v0.1.1-aarch64-apple-darwin.tar.gz`
- Windows: `jaturi-v0.1.1-x86_64-pc-windows-msvc.zip`

3. 압축 해제 후 실행

- Linux/macOS: `./jaturi`
- Windows: `jaturi.exe`

참고: 터미널 기반 앱(TUI)이므로 터미널에서 실행해야 합니다.

4. 무결성 검증(권장)

- 릴리스의 `SHA256SUMS.txt`를 함께 내려받아 체크섬을 비교하세요.

5. 플랫폼 보안 경고 참고

- macOS/Windows에서 서명되지 않은 바이너리 경고(Gatekeeper/SmartScreen)가 나타날 수 있습니다.

## 릴리스 자동 빌드

- 워크플로우 파일: `.github/workflows/release.yml`
- 트리거: `v*` 형태 태그 푸시 또는 수동 실행(`workflow_dispatch`)
- 산출물: Linux/macOS/Windows 실행 파일 + README 포함 압축본 + `SHA256SUMS.txt`

## 리네이밍 참고

- 서비스 이름이 `vocab_tui`에서 `jaturi`로 변경되었습니다.
- 앱 저장 데이터는 기존 `vocab_tui/state.bin`이 있으면 첫 실행 시 `jaturi/state.bin`으로 자동 마이그레이션됩니다.

## TUI 조작

- API Key 설정 화면
  - `Enter`: API Key 저장 후 메인 이동
  - `Esc`: 종료
- 주제 생성 화면
  - `Enter`: AI 주제/단어 생성 시작
  - `M`: 메인으로 이동
  - `Esc`: 종료
- 학습 화면
  - `Enter`: 다음 단어
  - `Q`: 바로 퀴즈 시작
- 퀴즈 화면
  - `Up/Down`: 보기 선택
  - `Enter`: 답 제출
- 결과/에러 화면
  - `R`: 처음(설정)으로 돌아가기
  - `Q`: 종료

## 입력 규칙

- Word Count는 사용자 입력 없이 `10`개로 고정
- API Key가 비어 있으면 요청하지 않음

## OpenAI 응답 형식

프로그램은 JSON Schema를 강제하여 아래 필드를 받습니다.

- `topic`
- `term`
- `meaning_ko`
