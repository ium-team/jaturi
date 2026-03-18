# Vocab TUI (Rust)

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
