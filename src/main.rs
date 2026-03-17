use std::collections::HashSet;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use rand::seq::SliceRandom;
use rand::Rng;
use ratatui::layout::{Constraint, Direction, Layout, Margin};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WordItem {
    term: String,
    meaning_ko: String,
}

#[derive(Debug, Deserialize)]
struct WordPayload {
    words: Vec<WordItem>,
}

#[derive(Debug, Serialize)]
struct ChatCompletionRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    response_format: ResponseFormat<'a>,
    temperature: f32,
}

#[derive(Debug, Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Debug, Serialize)]
struct ResponseFormat<'a> {
    r#type: &'a str,
    json_schema: JsonSchema<'a>,
}

#[derive(Debug, Serialize)]
struct JsonSchema<'a> {
    name: &'a str,
    schema: serde_json::Value,
    strict: bool,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: ChatMessageResponse,
}

#[derive(Debug, Deserialize)]
struct ChatMessageResponse {
    content: Option<String>,
}

#[derive(Debug, Clone)]
enum QuizAnswer {
    Choice(usize),
    Text(String),
}

#[derive(Debug, Clone, Copy)]
enum QuizType {
    MeaningChoice,
    FillBlankChoice,
    SpellingWrite,
}

#[derive(Debug, Clone)]
struct QuizQuestion {
    quiz_type: QuizType,
    target: String,
    prompt: String,
    options: Vec<String>,
    answer: QuizAnswer,
}

#[derive(Debug, Clone)]
struct QuizReviewItem {
    label: String,
    user_answer: String,
    correct_answer: String,
    is_correct: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    ApiKeySetup,
    Main,
    TopicCreate,
    Loading,
    Study,
    Quiz,
    Result,
    Error,
}

#[derive(Debug, Clone, Copy)]
enum InputField {
    ApiKey,
    Topic,
    Count,
}

impl InputField {
    fn next_for_topic_create(self) -> Self {
        match self {
            InputField::Topic => InputField::Count,
            _ => InputField::Topic,
        }
    }
}

#[derive(Debug)]
struct GenerationResult {
    topic: String,
    words: Vec<WordItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TopicRecord {
    topic: String,
    words: Vec<WordItem>,
    last_score: Option<(usize, usize)>,
    passed: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedState {
    version: u8,
    profile_name: String,
    total_xp: u32,
    english_skill: u32,
    topic_history: Vec<TopicRecord>,
    selected_topic: usize,
}

const STATE_VERSION: u8 = 2;
const STATE_DIR_NAME: &str = "vocab_tui";
const STATE_FILE_NAME: &str = "state.bin";

#[derive(Debug)]
struct App {
    screen: Screen,
    focused: InputField,
    api_key: String,
    topic: String,
    count_text: String,
    topic_history: Vec<TopicRecord>,
    selected_topic: usize,
    active_topic: Option<usize>,
    words: Vec<WordItem>,
    study_index: usize,
    quiz_questions: Vec<QuizQuestion>,
    quiz_reviews: Vec<QuizReviewItem>,
    quiz_index: usize,
    selected_option: usize,
    typed_answer: String,
    score: usize,
    profile_name: String,
    total_xp: u32,
    english_skill: u32,
    message: String,
    quit: bool,
}

impl Default for App {
    fn default() -> Self {
        let api_key = String::new();
        Self {
            screen: Screen::ApiKeySetup,
            focused: InputField::ApiKey,
            api_key,
            topic: String::new(),
            count_text: String::new(),
            topic_history: Vec::new(),
            selected_topic: 0,
            active_topic: None,
            words: Vec::new(),
            study_index: 0,
            quiz_questions: Vec::new(),
            quiz_reviews: Vec::new(),
            quiz_index: 0,
            selected_option: 0,
            typed_answer: String::new(),
            score: 0,
            profile_name: "학습자".to_string(),
            total_xp: 0,
            english_skill: 0,
            message: "API Key를 입력하고 Enter를 누르세요".to_string(),
            quit: false,
        }
    }
}

impl App {
    fn add_xp(&mut self, xp: u32) {
        self.total_xp = self.total_xp.saturating_add(xp);
    }

    fn save_persisted_state(&self) -> Result<()> {
        let path = state_file_path()?;
        let state = PersistedState {
            version: STATE_VERSION,
            profile_name: self.profile_name.clone(),
            total_xp: self.total_xp,
            english_skill: self.english_skill,
            topic_history: self.topic_history.clone(),
            selected_topic: self
                .selected_topic
                .min(self.topic_history.len().saturating_sub(1)),
        };
        let bytes = bincode::serde::encode_to_vec(&state, bincode::config::standard())
            .context("학습 상태 직렬화 실패")?;
        atomic_write(&path, &bytes).context("학습 상태 파일 저장 실패")
    }

    fn load_persisted_state(&mut self) -> Result<bool> {
        let path = state_file_path()?;
        if !path.exists() {
            return Ok(false);
        }

        let bytes = fs::read(&path).context("학습 상태 파일 읽기 실패")?;
        let (state, used): (PersistedState, usize) =
            bincode::serde::decode_from_slice(&bytes, bincode::config::standard())
                .context("학습 상태 파일 파싱 실패")?;
        if used != bytes.len() {
            bail!("학습 상태 파일 끝에 불필요한 데이터가 있습니다");
        }

        if state.version != STATE_VERSION {
            bail!(
                "지원하지 않는 상태 버전입니다: {}, 기대값: {}",
                state.version,
                STATE_VERSION
            );
        }

        self.profile_name = state.profile_name;
        self.total_xp = state.total_xp;
        self.english_skill = state.english_skill;
        self.topic_history = state.topic_history;
        if self.topic_history.is_empty() {
            self.selected_topic = 0;
        } else {
            self.selected_topic = state.selected_topic.min(self.topic_history.len() - 1);
        }

        Ok(true)
    }

    fn parse_count(&self) -> Result<usize> {
        let parsed = self
            .count_text
            .trim()
            .parse::<usize>()
            .context("개수는 숫자로 입력해야 합니다")?;
        if (5..=20).contains(&parsed) {
            Ok(parsed)
        } else {
            bail!("개수는 5~20 사이로 입력해 주세요")
        }
    }

    fn start_study(&mut self, words: Vec<WordItem>) {
        self.words = words;
        self.study_index = 0;
        self.screen = Screen::Study;
        self.message = "Enter: 다음 단어, Q: 퀴즈 시작".to_string();
    }

    fn setup_api_key(&mut self) {
        let normalized = self.api_key.trim().to_string();
        if normalized.is_empty() {
            self.message = "API Key를 입력해 주세요".to_string();
            return;
        }
        self.api_key = normalized;
        self.screen = Screen::Main;
        self.message = "N: 새 주제 생성, S: 학습, Q: 시험".to_string();
        self.focused = InputField::Topic;
    }

    fn handle_api_key_input(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Backspace => {
                self.api_key.pop();
            }
            KeyCode::Char(c) => {
                if !key.modifiers.contains(KeyModifiers::CONTROL) {
                    self.api_key.push(c);
                }
            }
            _ => {}
        }
    }

    fn handle_topic_create_input(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Tab => self.focused = self.focused.next_for_topic_create(),
            KeyCode::Backspace => match self.focused {
                InputField::Topic => {
                    self.topic.pop();
                }
                InputField::Count => {
                    self.count_text.pop();
                }
                InputField::ApiKey => {}
            },
            KeyCode::Char(c) => {
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    return;
                }
                match self.focused {
                    InputField::Topic => self.topic.push(c),
                    InputField::Count => {
                        if c.is_ascii_digit() {
                            self.count_text.push(c);
                        }
                    }
                    InputField::ApiKey => {}
                }
            }
            _ => {}
        }
    }

    fn topic_count(&self) -> usize {
        self.topic_history.len()
    }

    fn move_topic_selection(&mut self, delta: isize) {
        let len = self.topic_count();
        if len == 0 {
            self.selected_topic = 0;
            return;
        }
        let current = self.selected_topic.min(len - 1) as isize;
        let next = (current + delta).clamp(0, (len - 1) as isize) as usize;
        self.selected_topic = next;
    }

    fn normalize_selection(&mut self) {
        if self.topic_history.is_empty() {
            self.selected_topic = 0;
            self.active_topic = None;
            return;
        }
        self.selected_topic = self.selected_topic.min(self.topic_history.len() - 1);
    }

    fn save_topic(&mut self, topic: String, words: Vec<WordItem>) -> usize {
        self.topic_history.push(TopicRecord {
            topic,
            words,
            last_score: None,
            passed: false,
        });
        let index = self.topic_history.len() - 1;
        self.selected_topic = index;
        if let Err(err) = self.save_persisted_state() {
            self.message = format!("학습 상태 저장 실패: {err}");
        }
        index
    }

    fn start_study_for(&mut self, index: usize) {
        if let Some(words) = self.topic_history.get(index).map(|record| record.words.clone()) {
            self.active_topic = Some(index);
            self.start_study(words);
        }
    }

    fn start_quiz_for(&mut self, index: usize) {
        if let Some(words) = self.topic_history.get(index).map(|record| record.words.clone()) {
            self.active_topic = Some(index);
            self.words = words;
            self.start_quiz();
        }
    }

    fn finish_quiz(&mut self) {
        if let Some(active) = self.active_topic {
            let total = self.quiz_questions.len();
            if total > 0 {
                let passed = self.score * 100 >= total * 70;
                self.add_xp(5);
                if passed {
                    self.english_skill = self.english_skill.saturating_add(1);
                }
                if let Some(record) = self.topic_history.get_mut(active) {
                    record.last_score = Some((self.score, total));
                    record.passed = passed;
                }
                self.message = if passed {
                    "복습 완료! +5 XP, 영어 실력 +1".to_string()
                } else {
                    "복습 완료! +5 XP".to_string()
                };
            }
        }
        self.screen = Screen::Result;
        if let Err(err) = self.save_persisted_state() {
            self.message = format!("학습 상태 저장 실패: {err}");
        }
    }

    fn start_main(&mut self) {
        self.normalize_selection();
        self.screen = Screen::Main;
        self.message = "N: 새 주제 생성, S: 학습, Q: 시험, Enter: 학습".to_string();
    }

    fn selected_topic_record(&self) -> Option<&TopicRecord> {
        self.topic_history.get(self.selected_topic)
    }

    fn known_terms(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut terms = Vec::new();

        for record in &self.topic_history {
            for word in &record.words {
                let key = normalize_term(&word.term);
                if !key.is_empty() && seen.insert(key) {
                    terms.push(word.term.clone());
                }
            }
        }

        terms
    }

    fn start_quiz(&mut self) {
        self.quiz_questions = build_quiz_questions(&self.words);
        self.quiz_reviews.clear();
        self.quiz_index = 0;
        self.selected_option = 0;
        self.typed_answer.clear();
        self.score = 0;
        self.screen = Screen::Quiz;
        self.message = "문제 유형이 랜덤으로 출제됩니다".to_string();
    }

    fn current_quiz(&self) -> Option<&QuizQuestion> {
        self.quiz_questions.get(self.quiz_index)
    }

    fn answer_current(&mut self) {
        if let Some(question) = self.current_quiz().cloned() {
            let (is_correct, user_answer, correct_answer) = match &question.answer {
                QuizAnswer::Choice(answer_index) => {
                    let user = question
                        .options
                        .get(self.selected_option)
                        .cloned()
                        .unwrap_or_else(|| "(미선택)".to_string());
                    let correct = question
                        .options
                        .get(*answer_index)
                        .cloned()
                        .unwrap_or_else(|| "(정답 없음)".to_string());
                    (self.selected_option == *answer_index, user, correct)
                }
                QuizAnswer::Text(answer_text) => {
                    let user = if self.typed_answer.trim().is_empty() {
                        "(미입력)".to_string()
                    } else {
                        self.typed_answer.trim().to_string()
                    };
                    (
                        is_case_flexible_exact_match(&self.typed_answer, answer_text),
                        user,
                        answer_text.clone(),
                    )
                }
            };
            if is_correct {
                self.score += 1;
            }
            self.quiz_reviews.push(QuizReviewItem {
                label: format!("{}: {}", quiz_type_label(question.quiz_type), question.target),
                user_answer,
                correct_answer,
                is_correct,
            });
            self.quiz_index += 1;
            self.selected_option = 0;
            self.typed_answer.clear();
            if self.quiz_index >= self.quiz_questions.len() {
                self.finish_quiz();
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let terminal = setup_terminal().context("터미널 초기화 실패")?;
    let result = run(terminal).await;
    restore_terminal().context("터미널 복구 실패")?;
    result
}

async fn run(mut terminal: DefaultTerminal) -> Result<()> {
    let mut app = App::default();
    match app.load_persisted_state() {
        Ok(true) => {
            app.screen = Screen::Main;
            app.focused = InputField::Topic;
            app.message =
                "이전 학습 기록을 불러왔습니다. N으로 새 주제 생성 시 API Key를 입력해 주세요"
                    .to_string();
        }
        Ok(false) => {}
        Err(err) => {
            app.message = format!("저장된 학습 기록을 불러오지 못했습니다: {err}");
        }
    }
    let (tx, mut rx) = mpsc::unbounded_channel::<Result<GenerationResult>>();

    while !app.quit {
        terminal.draw(|frame| draw(frame, &app))?;

        if app.screen == Screen::Loading {
            match rx.try_recv() {
                Ok(result) => match result {
                    Ok(output) => {
                        let index = app.save_topic(output.topic, output.words);
                        app.start_study_for(index);
                    }
                    Err(err) => {
                        app.screen = Screen::Error;
                        app.message = format!("생성 실패: {err}");
                    }
                },
                Err(mpsc::error::TryRecvError::Empty) => {}
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    app.screen = Screen::Error;
                    app.message = "내부 채널이 끊어졌습니다".to_string();
                }
            }
        }

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                handle_key_event(key, &mut app, &tx).await;
            }
        }
    }

    if let Err(err) = app.save_persisted_state() {
        app.message = format!("학습 상태 저장 실패: {err}");
    }

    Ok(())
}

async fn handle_key_event(
    key: KeyEvent,
    app: &mut App,
    tx: &mpsc::UnboundedSender<Result<GenerationResult>>,
) {
    if key.code == KeyCode::Esc {
        app.quit = true;
        return;
    }

    match app.screen {
        Screen::ApiKeySetup => match key.code {
            KeyCode::Enter => {
                app.setup_api_key();
            }
            _ => app.handle_api_key_input(key),
        },
        Screen::Main => match key.code {
            KeyCode::Up => app.move_topic_selection(-1),
            KeyCode::Down => app.move_topic_selection(1),
            KeyCode::Enter | KeyCode::Char('s') | KeyCode::Char('S') => {
                if app.selected_topic_record().is_some() {
                    app.start_study_for(app.selected_topic);
                } else {
                    app.screen = Screen::TopicCreate;
                    app.focused = InputField::Topic;
                    app.message = "새 주제를 입력하세요".to_string();
                }
            }
            KeyCode::Char('q') | KeyCode::Char('Q') => {
                if app.selected_topic_record().is_some() {
                    app.start_quiz_for(app.selected_topic);
                }
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                app.screen = Screen::TopicCreate;
                app.focused = InputField::Topic;
                app.message = "주제/개수 입력 후 Enter: 단어 생성".to_string();
            }
            _ => {}
        },
        Screen::TopicCreate => match key.code {
            KeyCode::Enter => {
                let topic = app.topic.trim().to_string();
                if topic.is_empty() {
                    app.message = "주제를 입력해 주세요".to_string();
                    return;
                }
                let count = match app.parse_count() {
                    Ok(value) => value,
                    Err(err) => {
                        app.message = err.to_string();
                        return;
                    }
                };
                if app.api_key.trim().is_empty() {
                    app.screen = Screen::ApiKeySetup;
                    app.focused = InputField::ApiKey;
                    app.message = "API Key를 먼저 설정해 주세요".to_string();
                    return;
                }

                app.screen = Screen::Loading;
                app.message = "OpenAI에서 단어를 생성하는 중...".to_string();

                let api_key = app.api_key.clone();
                let excluded_terms = app.known_terms();
                let english_skill = app.english_skill;
                let tx = tx.clone();
                tokio::spawn(async move {
                    let result = fetch_words(&api_key, &topic, count, &excluded_terms, english_skill)
                        .await
                        .map(|words| GenerationResult { topic, words });
                    let _ = tx.send(result);
                });
            }
            KeyCode::Char('m') | KeyCode::Char('M') => app.start_main(),
            _ => app.handle_topic_create_input(key),
        },
        Screen::Loading => {}
        Screen::Study => match key.code {
            KeyCode::Enter => {
                if app.study_index + 1 < app.words.len() {
                    app.study_index += 1;
                } else {
                    app.add_xp(10);
                    if let Err(err) = app.save_persisted_state() {
                        app.message = format!("학습 상태 저장 실패: {err}");
                    }
                    app.start_quiz();
                }
            }
            KeyCode::Char('q') | KeyCode::Char('Q') => app.start_quiz(),
            _ => {}
        },
        Screen::Quiz => match key.code {
            KeyCode::Up => {
                let is_choice = matches!(
                    app.current_quiz().map(|question| &question.answer),
                    Some(QuizAnswer::Choice(_))
                );
                if is_choice && app.selected_option > 0 {
                    app.selected_option -= 1;
                }
            }
            KeyCode::Down => {
                let choice_option_len = app.current_quiz().and_then(|question| {
                    if matches!(question.answer, QuizAnswer::Choice(_)) {
                        Some(question.options.len())
                    } else {
                        None
                    }
                });
                if let Some(option_len) = choice_option_len {
                    if app.selected_option + 1 < option_len {
                        app.selected_option += 1;
                    }
                }
            }
            KeyCode::Enter => app.answer_current(),
            KeyCode::Backspace => {
                let is_text = matches!(
                    app.current_quiz().map(|question| &question.answer),
                    Some(QuizAnswer::Text(_))
                );
                if is_text {
                    app.typed_answer.pop();
                }
            }
            KeyCode::Char(c) => {
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    return;
                }
                let is_text = matches!(
                    app.current_quiz().map(|question| &question.answer),
                    Some(QuizAnswer::Text(_))
                );
                if is_text {
                    if c.is_ascii_alphabetic() || c == ' ' || c == '-' || c == '\'' {
                        app.typed_answer.push(c);
                    }
                }
            }
            _ => {}
        },
        Screen::Result => match key.code {
            KeyCode::Char('m') | KeyCode::Char('M') => app.start_main(),
            KeyCode::Char('s') | KeyCode::Char('S') => {
                if let Some(active) = app.active_topic {
                    app.start_study_for(active);
                }
            }
            KeyCode::Char('q') | KeyCode::Char('Q') => {
                if let Some(active) = app.active_topic {
                    app.start_quiz_for(active);
                }
            }
            _ => {}
        },
        Screen::Error => match key.code {
            KeyCode::Char('r') | KeyCode::Char('R') => {
                if app.api_key.trim().is_empty() {
                    app.screen = Screen::ApiKeySetup;
                    app.focused = InputField::ApiKey;
                } else {
                    app.start_main();
                }
            }
            KeyCode::Char('q') | KeyCode::Char('Q') => app.quit = true,
            _ => {}
        },
    }
}

fn draw(frame: &mut Frame<'_>, app: &App) {
    match app.screen {
        Screen::ApiKeySetup => draw_api_key_setup(frame, app),
        Screen::Main => draw_main(frame, app),
        Screen::TopicCreate => draw_topic_create(frame, app),
        Screen::Loading => draw_loading(frame, app),
        Screen::Study => draw_study(frame, app),
        Screen::Quiz => draw_quiz(frame, app),
        Screen::Result => draw_result(frame, app),
        Screen::Error => draw_error(frame, app),
    }
}

fn draw_api_key_setup(frame: &mut Frame<'_>, app: &App) {
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(1),
        ])
        .margin(2)
        .split(frame.area());

    let title = Paragraph::new("English Vocab TUI")
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .block(Block::default().borders(Borders::ALL).title("API Key Setup"));

    let api_value = if app.api_key.is_empty() {
        "(input your OpenAI API key)".to_string()
    } else {
        "*".repeat(app.api_key.len().min(40))
    };
    let api_style = if matches!(app.focused, InputField::ApiKey) {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    let api = Paragraph::new(api_value)
        .style(api_style)
        .block(Block::default().borders(Borders::ALL).title("API Key"));

    let help = Paragraph::new(vec![
        Line::from("Enter: API Key 저장 후 메인 이동"),
        Line::from("Esc: 종료"),
        Line::from(app.message.clone()),
    ])
    .block(Block::default().borders(Borders::ALL).title("Help"))
    .wrap(Wrap { trim: true });

    frame.render_widget(title, areas[0]);
    frame.render_widget(api, areas[1]);
    frame.render_widget(help, areas[2]);
}

fn draw_main(frame: &mut Frame<'_>, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(7),
            Constraint::Min(8),
            Constraint::Length(6),
        ])
        .margin(2)
        .split(frame.area());

    let title = Paragraph::new("Main Menu")
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .block(Block::default().borders(Borders::ALL).title("English Vocab TUI"));

    let items: Vec<ListItem<'_>> = if app.topic_history.is_empty() {
        vec![ListItem::new("저장된 주제가 없습니다. N으로 새 주제를 생성하세요.")]
    } else {
        app.topic_history
            .iter()
            .enumerate()
            .map(|(idx, record)| {
                let status = if record.passed { "passed" } else { "in progress" };
                let score = record
                    .last_score
                    .map(|(value, total)| format!("  score: {value}/{total}"))
                    .unwrap_or_default();
                let line = format!(
                    "{}topic: {}  words: {}  status: {}{}",
                    if idx == app.selected_topic { "> " } else { "  " },
                    record.topic,
                    record.words.len(),
                    status,
                    score
                );
                ListItem::new(line)
            })
            .collect()
    };

    let topic_list = List::new(items).block(Block::default().borders(Borders::ALL).title("Review Topics"));

    let (level, current_level_xp, current_level_required, next_level_remaining) =
        level_progress_from_xp(app.total_xp);

    let profile = Paragraph::new(vec![
        Line::from(format!("이름: {}", app.profile_name)),
        Line::from(format!(
            "전체 XP: {} (다음 레벨까지 {} XP)",
            app.total_xp, next_level_remaining
        )),
        Line::from(format!(
            "레벨: {} (레벨 진행 {}/{})",
            level, current_level_xp, current_level_required
        )),
        Line::from(format!("언어 실력(영어): {}", app.english_skill)),
    ])
    .block(Block::default().borders(Borders::ALL).title("Profile"))
    .wrap(Wrap { trim: true });

    let help = Paragraph::new(vec![
        Line::from("N: 새 주제 생성"),
        Line::from("Up/Down: 과거 주제 선택"),
        Line::from("S/Enter: 단어 학습 시작, Q: 시험 보기"),
        Line::from("Esc: 종료"),
        Line::from(app.message.clone()),
    ])
    .block(Block::default().borders(Borders::ALL).title("Actions"))
    .wrap(Wrap { trim: true });

    frame.render_widget(title, chunks[0]);
    frame.render_widget(profile, chunks[1]);
    frame.render_widget(topic_list, chunks[2]);
    frame.render_widget(help, chunks[3]);
}

fn xp_required_for_next_level(level: u32) -> u32 {
    let base: u32 = 120;
    let growth_per_level: u32 = 30;
    base + growth_per_level.saturating_mul(level.saturating_sub(1))
}

fn level_progress_from_xp(total_xp: u32) -> (u32, u32, u32, u32) {
    let mut level = 1;
    let mut remaining_xp = total_xp;

    loop {
        let required = xp_required_for_next_level(level);
        if remaining_xp < required {
            let xp_in_current_level = remaining_xp;
            let xp_to_next_level = required - remaining_xp;
            return (level, xp_in_current_level, required, xp_to_next_level);
        }
        remaining_xp -= required;
        level = level.saturating_add(1);
    }
}

fn draw_topic_create(frame: &mut Frame<'_>, app: &App) {
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(4),
            Constraint::Min(1),
        ])
        .margin(2)
        .split(frame.area());

    let title = Paragraph::new("새 주제 생성")
        .style(
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )
        .block(Block::default().borders(Borders::ALL).title("Create Topic"));

    let topic_style = if matches!(app.focused, InputField::Topic) {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    let topic_value = if app.topic.is_empty() {
        "(예: travel conversation)".to_string()
    } else {
        app.topic.clone()
    };
    let topic = Paragraph::new(topic_value)
        .style(topic_style)
        .block(Block::default().borders(Borders::ALL).title("Topic"));

    let count_style = if matches!(app.focused, InputField::Count) {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    let count_value = if app.count_text.is_empty() {
        "(5-20)".to_string()
    } else {
        app.count_text.clone()
    };
    let count = Paragraph::new(count_value)
        .style(count_style)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Word Count (5-20)"),
        );

    let help = Paragraph::new(vec![
        Line::from("Tab: Topic/Count 이동"),
        Line::from("Enter: 단어 생성 시작"),
        Line::from("M: 메인으로 이동"),
        Line::from("Esc: 종료"),
        Line::from(app.message.clone()),
    ])
    .block(Block::default().borders(Borders::ALL).title("Help"))
    .wrap(Wrap { trim: true });

    frame.render_widget(title, areas[0]);
    frame.render_widget(topic, areas[1]);
    frame.render_widget(count, areas[2]);
    frame.render_widget(help, areas[3]);
}

fn draw_loading(frame: &mut Frame<'_>, app: &App) {
    let popup = centered_rect(60, 20, frame.area());
    frame.render_widget(Clear, popup);
    let loading = Paragraph::new(vec![
        Line::from("OpenAI로 단어를 생성하는 중입니다."),
        Line::from("잠시만 기다려 주세요..."),
        Line::from(" "),
        Line::from(app.message.clone()),
    ])
    .block(Block::default().borders(Borders::ALL).title("Loading"))
    .wrap(Wrap { trim: true });
    frame.render_widget(loading, popup);
}

fn draw_study(frame: &mut Frame<'_>, app: &App) {
    let Some(word) = app.words.get(app.study_index) else {
        return;
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(5),
            Constraint::Min(1),
        ])
        .margin(2)
        .split(frame.area());

    let title = Paragraph::new(format!(
        "Study Mode ({}/{})",
        app.study_index + 1,
        app.words.len()
    ))
    .style(
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
    )
    .block(Block::default().borders(Borders::ALL).title("Progress"));

    let term = Paragraph::new(word.term.clone())
        .style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .block(Block::default().borders(Borders::ALL).title("Word"));

    let meaning = Paragraph::new(word.meaning_ko.clone())
        .block(Block::default().borders(Borders::ALL).title("Meaning (KR)"));

    let help = Paragraph::new(vec![
        Line::from("Enter: 다음 단어"),
        Line::from("Q: 바로 퀴즈 시작"),
        Line::from("Esc: 종료"),
    ])
    .block(Block::default().borders(Borders::ALL).title("Help"));

    frame.render_widget(title, chunks[0]);
    frame.render_widget(term, chunks[1]);
    frame.render_widget(meaning, chunks[2]);
    frame.render_widget(help, chunks[3]);
}

fn draw_quiz(frame: &mut Frame<'_>, app: &App) {
    let Some(question) = app.current_quiz() else {
        return;
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(5),
            Constraint::Min(1),
        ])
        .margin(2)
        .split(frame.area());

    let title = Paragraph::new(format!(
        "Quiz ({}/{})  Score: {}",
        app.quiz_index + 1,
        app.quiz_questions.len(),
        app.score
    ))
    .style(
        Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!("Quiz Mode - {}", quiz_type_label(question.quiz_type))),
    );

    let prompt = Paragraph::new(question.prompt.clone())
        .block(Block::default().borders(Borders::ALL).title("Question"));

    frame.render_widget(title, chunks[0]);
    frame.render_widget(prompt, chunks[1]);

    match &question.answer {
        QuizAnswer::Choice(_) => {
            let items: Vec<ListItem<'_>> = question
                .options
                .iter()
                .enumerate()
                .map(|(idx, option)| {
                    let style = if idx == app.selected_option {
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::Yellow)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    };
                    ListItem::new(option.as_str()).style(style)
                })
                .collect();

            let list = List::new(items).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Up/Down으로 선택, Enter 제출"),
            );
            frame.render_widget(list, chunks[2]);
        }
        QuizAnswer::Text(_) => {
            let answer = Paragraph::new(vec![
                Line::from(format!("입력: {}", app.typed_answer)),
                Line::from(" "),
                Line::from("알파벳 입력 + Backspace, Enter 제출"),
            ])
            .block(Block::default().borders(Borders::ALL).title("Type Answer"))
            .wrap(Wrap { trim: true });
            frame.render_widget(answer, chunks[2]);
        }
    }
}

fn draw_result(frame: &mut Frame<'_>, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(6),
            Constraint::Length(6),
        ])
        .margin(2)
        .split(frame.area());

    let rate = if app.quiz_questions.is_empty() {
        0.0
    } else {
        (app.score as f64 / app.quiz_questions.len() as f64) * 100.0
    };

    let title = Paragraph::new("Quiz Result")
        .style(
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )
        .block(Block::default().borders(Borders::ALL).title("Result"));

    let score = Paragraph::new(vec![
        Line::from(format!("정답: {}/{}", app.score, app.quiz_questions.len())),
        Line::from(format!("정답률: {:.1}%", rate)),
    ])
    .block(Block::default().borders(Borders::ALL).title("Score"));

    let review_items: Vec<ListItem<'_>> = if app.quiz_reviews.is_empty() {
        vec![ListItem::new("문제 결과가 없습니다")]
    } else {
        app.quiz_reviews
            .iter()
            .enumerate()
            .map(|(idx, item)| {
                let mark = if item.is_correct { "O" } else { "X" };
                ListItem::new(format!(
                    "{}. [{}] {} | 내 답: {} | 정답: {}",
                    idx + 1,
                    mark,
                    item.label,
                    item.user_answer,
                    item.correct_answer
                ))
            })
            .collect()
    };

    let review = List::new(review_items)
        .block(Block::default().borders(Borders::ALL).title("문제별 정오답"));

    let help = Paragraph::new(vec![
        Line::from("M: 메인으로 이동"),
        Line::from("S: 다시 학습하기"),
        Line::from("Q: 다시 시험보기"),
        Line::from("Esc: 종료"),
    ])
    .block(Block::default().borders(Borders::ALL).title("Actions"))
    .wrap(Wrap { trim: true });

    frame.render_widget(title, chunks[0]);
    frame.render_widget(score, chunks[1]);
    frame.render_widget(review, chunks[2]);
    frame.render_widget(help, chunks[3]);
}

fn draw_error(frame: &mut Frame<'_>, app: &App) {
    let popup = centered_rect(70, 35, frame.area());
    frame.render_widget(Clear, popup);

    let text = Paragraph::new(vec![
        Line::from(app.message.clone()),
        Line::from(" "),
        Line::from("R: 돌아가기"),
        Line::from("Q: 종료"),
    ])
    .style(Style::default().fg(Color::Red))
    .block(Block::default().borders(Borders::ALL).title("Error"))
    .wrap(Wrap { trim: true });

    frame.render_widget(text, popup);
}

fn centered_rect(
    percent_x: u16,
    percent_y: u16,
    area: ratatui::layout::Rect,
) -> ratatui::layout::Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1]);
    horizontal[1].inner(Margin {
        vertical: 0,
        horizontal: 0,
    })
}

fn quiz_type_label(quiz_type: QuizType) -> &'static str {
    match quiz_type {
        QuizType::MeaningChoice => "뜻 맞추기",
        QuizType::FillBlankChoice => "빈칸 채우기",
        QuizType::SpellingWrite => "철자 입력",
    }
}

fn is_case_flexible_exact_match(input: &str, answer: &str) -> bool {
    input.trim().eq_ignore_ascii_case(answer.trim())
}

fn mask_term(term: &str) -> String {
    let chars: Vec<char> = term.chars().collect();
    if chars.len() <= 2 {
        return "__".to_string();
    }
    let mut masked = String::new();
    for (idx, ch) in chars.iter().enumerate() {
        if idx == 0 || idx + 1 == chars.len() {
            masked.push(*ch);
        } else if ch.is_ascii_alphabetic() {
            masked.push('_');
        } else {
            masked.push(*ch);
        }
    }
    masked
}

fn unique_terms(words: &[WordItem], exclude_index: usize) -> Vec<String> {
    let mut items: Vec<String> = words
        .iter()
        .enumerate()
        .filter(|(idx, _)| *idx != exclude_index)
        .map(|(_, item)| item.term.clone())
        .collect();
    items.sort();
    items.dedup();
    items
}

fn unique_meanings(words: &[WordItem], exclude_index: usize) -> Vec<String> {
    let mut items: Vec<String> = words
        .iter()
        .enumerate()
        .filter(|(idx, _)| *idx != exclude_index)
        .map(|(_, item)| item.meaning_ko.clone())
        .collect();
    items.sort();
    items.dedup();
    items
}

fn build_quiz_questions(words: &[WordItem]) -> Vec<QuizQuestion> {
    let mut rng = rand::rng();
    let mut questions = Vec::with_capacity(words.len());

    for (index, word) in words.iter().enumerate() {
        let question_type = match rng.random_range(0..3) {
            0 => QuizType::MeaningChoice,
            1 => QuizType::FillBlankChoice,
            _ => QuizType::SpellingWrite,
        };

        let question = match question_type {
            QuizType::MeaningChoice => {
                let mut wrong_meanings = unique_meanings(words, index);
                wrong_meanings.shuffle(&mut rng);

                let mut options = vec![word.meaning_ko.clone()];
                options.extend(wrong_meanings.into_iter().take(3));
                options.shuffle(&mut rng);

                let answer_index = options
                    .iter()
                    .position(|option| option == &word.meaning_ko)
                    .unwrap_or(0);

                QuizQuestion {
                    quiz_type: QuizType::MeaningChoice,
                    target: word.term.clone(),
                    prompt: format!("'{}'의 뜻을 고르세요", word.term),
                    options,
                    answer: QuizAnswer::Choice(answer_index),
                }
            }
            QuizType::FillBlankChoice => {
                let mut wrong_terms = unique_terms(words, index);
                wrong_terms.shuffle(&mut rng);

                let mut options = vec![word.term.clone()];
                options.extend(wrong_terms.into_iter().take(3));
                options.shuffle(&mut rng);

                let answer_index = options
                    .iter()
                    .position(|option| option == &word.term)
                    .unwrap_or(0);

                let prompt = format!(
                    "빈칸에 들어갈 단어를 고르세요\n뜻: {}\n철자 힌트: {}",
                    word.meaning_ko,
                    mask_term(&word.term)
                );

                QuizQuestion {
                    quiz_type: QuizType::FillBlankChoice,
                    target: word.term.clone(),
                    prompt,
                    options,
                    answer: QuizAnswer::Choice(answer_index),
                }
            }
            QuizType::SpellingWrite => QuizQuestion {
                quiz_type: QuizType::SpellingWrite,
                target: word.term.clone(),
                prompt: format!(
                    "뜻을 보고 단어를 직접 입력하세요\n뜻: {}",
                    word.meaning_ko
                ),
                options: Vec::new(),
                answer: QuizAnswer::Text(word.term.clone()),
            },
        };

        questions.push(question);
    }

    questions.shuffle(&mut rng);
    questions
}

fn normalize_term(term: &str) -> String {
    term.trim().to_ascii_lowercase()
}

fn is_single_word_term(term: &str) -> bool {
    let trimmed = term.trim();
    !trimmed.is_empty()
        && trimmed
            .chars()
            .all(|ch| ch.is_ascii_alphabetic() || ch == '-' || ch == '\'')
        && !trimmed.contains(' ')
}

async fn fetch_words(
    api_key: &str,
    topic: &str,
    count: usize,
    excluded_terms: &[String],
    english_skill: u32,
) -> Result<Vec<WordItem>> {
    let mut blocked = HashSet::new();
    for term in excluded_terms {
        let normalized = normalize_term(term);
        if !normalized.is_empty() {
            blocked.insert(normalized);
        }
    }

    let mut collected: Vec<WordItem> = Vec::with_capacity(count);
    const MAX_ATTEMPTS: usize = 6;

    for _ in 0..MAX_ATTEMPTS {
        if collected.len() >= count {
            break;
        }

        let remaining = count - collected.len();
        let batch_count = remaining.saturating_add(4);
        let batch = fetch_words_batch(api_key, topic, batch_count, english_skill).await?;
        for item in batch {
            let key = normalize_term(&item.term);
            if key.is_empty() || blocked.contains(&key) || !is_single_word_term(&item.term) {
                continue;
            }
            blocked.insert(key);
            collected.push(item);
            if collected.len() >= count {
                break;
            }
        }
    }

    if collected.len() != count {
        bail!(
            "중복 없는 새 단어를 충분히 만들지 못했습니다: 요청 {count}, 생성 {}",
            collected.len()
        );
    }

    Ok(collected)
}

async fn fetch_words_batch(
    api_key: &str,
    topic: &str,
    count: usize,
    english_skill: u32,
) -> Result<Vec<WordItem>> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "words": {
                "type": "array",
                "minItems": count,
                "maxItems": count,
                "items": {
                    "type": "object",
                    "properties": {
                        "term": {
                            "type": "string",
                            "pattern": "^[A-Za-z]+(?:[-'][A-Za-z]+)*$"
                        },
                        "meaning_ko": {"type": "string"}
                    },
                    "required": ["term", "meaning_ko"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["words"],
        "additionalProperties": false
    });

    let user_prompt = format!(
        "주제: {topic}\n현재 사용자 영어 실력 레벨: {english_skill}\n주제에 맞는 실용 영어 단어를 정확히 {count}개 생성하세요. term은 반드시 한 단어만 허용되며 공백이 들어간 문장/구는 절대 금지입니다. 각 항목은 term(영단어)과 meaning_ko(짧고 명확한 한국어 뜻)만 포함하세요."
    );

    let request_body = ChatCompletionRequest {
        model: "gpt-4o-mini",
        messages: vec![
            ChatMessage {
                role: "system",
                content: "You are a fast vocabulary generator for Korean learners. Return only JSON that matches the schema. Every term must be exactly one English word (no spaces, no phrases, no full sentences). Difficulty should adapt to the user's English skill level.",
            },
            ChatMessage {
                role: "user",
                content: &user_prompt,
            },
        ],
        response_format: ResponseFormat {
            r#type: "json_schema",
            json_schema: JsonSchema {
                name: "word_payload",
                schema,
                strict: true,
            },
        },
        temperature: 0.2,
    };

    let client = reqwest::Client::new();
    let response = client
        .post("https://api.openai.com/v1/chat/completions")
        .bearer_auth(api_key)
        .json(&request_body)
        .send()
        .await
        .context("OpenAI 요청 실패")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "응답 본문 읽기 실패".to_string());
        bail!("OpenAI API 오류({status}): {body}");
    }

    let parsed: ChatCompletionResponse = response
        .json()
        .await
        .context("OpenAI 응답 JSON 파싱 실패")?;

    let content = parsed
        .choices
        .first()
        .and_then(|choice| choice.message.content.clone())
        .ok_or_else(|| anyhow!("OpenAI 응답에 content가 없습니다"))?;

    let payload: WordPayload = serde_json::from_str(&content).context("단어 JSON 파싱 실패")?;
    if payload.words.len() != count {
        bail!(
            "요청한 단어 수와 응답 단어 수가 다릅니다: 요청 {count}, 응답 {}",
            payload.words.len()
        );
    }
    Ok(payload.words)
}

fn state_file_path() -> Result<PathBuf> {
    let base_dir = if let Ok(xdg_data_home) = env::var("XDG_DATA_HOME") {
        PathBuf::from(xdg_data_home)
    } else if let Ok(home) = env::var("HOME") {
        PathBuf::from(home).join(".local").join("share")
    } else {
        bail!("상태 저장 경로를 확인할 수 없습니다(HOME/XDG_DATA_HOME 없음)")
    };

    Ok(base_dir.join(STATE_DIR_NAME).join(STATE_FILE_NAME))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("상태 저장 경로가 올바르지 않습니다"))?;
    fs::create_dir_all(parent).context("상태 저장 디렉터리 생성 실패")?;

    let tmp_path = path.with_extension("tmp");
    {
        let mut tmp_file = fs::File::create(&tmp_path).context("임시 상태 파일 생성 실패")?;
        tmp_file
            .write_all(bytes)
            .context("임시 상태 파일 쓰기 실패")?;
        tmp_file.sync_all().context("임시 상태 파일 동기화 실패")?;
    }
    fs::rename(&tmp_path, path).context("상태 파일 교체 실패")?;
    fs::File::open(parent)
        .and_then(|dir| dir.sync_all())
        .context("상태 파일 디렉터리 동기화 실패")
}

fn setup_terminal() -> Result<DefaultTerminal> {
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen)?;
    let terminal = ratatui::init();
    Ok(terminal)
}

fn restore_terminal() -> Result<()> {
    ratatui::restore();
    execute!(io::stdout(), LeaveAlternateScreen)?;
    disable_raw_mode()?;
    Ok(())
}
