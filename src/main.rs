use std::io;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use rand::seq::SliceRandom;
use ratatui::layout::{Constraint, Direction, Layout, Margin};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WordItem {
    term: String,
    meaning_ko: String,
    example_en: String,
    example_ko: String,
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
struct QuizQuestion {
    word: String,
    options: Vec<String>,
    answer_index: usize,
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
    count: usize,
    words: Vec<WordItem>,
}

#[derive(Debug, Clone)]
struct TopicRecord {
    topic: String,
    count: usize,
    words: Vec<WordItem>,
    last_score: Option<(usize, usize)>,
    passed: bool,
}

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
    quiz_index: usize,
    selected_option: usize,
    score: usize,
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
            topic: "daily conversation".to_string(),
            count_text: "10".to_string(),
            topic_history: Vec::new(),
            selected_topic: 0,
            active_topic: None,
            words: Vec::new(),
            study_index: 0,
            quiz_questions: Vec::new(),
            quiz_index: 0,
            selected_option: 0,
            score: 0,
            message: "API Key를 입력하고 Enter를 누르세요".to_string(),
            quit: false,
        }
    }
}

impl App {
    fn parse_count(&self) -> Result<usize> {
        let parsed = self
            .count_text
            .trim()
            .parse::<usize>()
            .context("개수는 숫자로 입력해야 합니다")?;
        if (5..=30).contains(&parsed) {
            Ok(parsed)
        } else {
            bail!("개수는 5~30 사이로 입력해 주세요")
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

    fn save_topic(&mut self, topic: String, count: usize, words: Vec<WordItem>) -> usize {
        self.topic_history.push(TopicRecord {
            topic,
            count,
            words,
            last_score: None,
            passed: false,
        });
        let index = self.topic_history.len() - 1;
        self.selected_topic = index;
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
                if let Some(record) = self.topic_history.get_mut(active) {
                    record.last_score = Some((self.score, total));
                    record.passed = passed;
                }
            }
        }
        self.screen = Screen::Result;
        self.message = "M: 메인, S: 학습, Q: 시험, Esc: 종료".to_string();
    }

    fn start_main(&mut self) {
        self.normalize_selection();
        self.screen = Screen::Main;
        self.message = "N: 새 주제 생성, S: 학습, Q: 시험, Enter: 학습".to_string();
    }

    fn selected_topic_record(&self) -> Option<&TopicRecord> {
        self.topic_history.get(self.selected_topic)
    }

    fn start_quiz(&mut self) {
        self.quiz_questions = build_quiz_questions(&self.words);
        self.quiz_index = 0;
        self.selected_option = 0;
        self.score = 0;
        self.screen = Screen::Quiz;
        self.message = "위/아래로 선택, Enter로 제출".to_string();
    }

    fn current_quiz(&self) -> Option<&QuizQuestion> {
        self.quiz_questions.get(self.quiz_index)
    }

    fn answer_current(&mut self) {
        if let Some(question) = self.current_quiz() {
            if self.selected_option == question.answer_index {
                self.score += 1;
            }
            self.quiz_index += 1;
            self.selected_option = 0;
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
    let (tx, mut rx) = mpsc::unbounded_channel::<Result<GenerationResult>>();

    while !app.quit {
        terminal.draw(|frame| draw(frame, &app))?;

        if app.screen == Screen::Loading {
            match rx.try_recv() {
                Ok(result) => match result {
                    Ok(output) => {
                        let index = app.save_topic(output.topic, output.count, output.words);
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
                app.message = "주제 입력 후 Enter: 단어 생성".to_string();
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
                let tx = tx.clone();
                tokio::spawn(async move {
                    let result = fetch_words(&api_key, &topic, count).await.map(|words| GenerationResult {
                        topic,
                        count,
                        words,
                    });
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
                    app.start_quiz();
                }
            }
            KeyCode::Char('q') | KeyCode::Char('Q') => app.start_quiz(),
            _ => {}
        },
        Screen::Quiz => match key.code {
            KeyCode::Up => {
                if app.selected_option > 0 {
                    app.selected_option -= 1;
                }
            }
            KeyCode::Down => {
                if let Some(question) = app.current_quiz() {
                    if app.selected_option + 1 < question.options.len() {
                        app.selected_option += 1;
                    }
                }
            }
            KeyCode::Enter => app.answer_current(),
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
                    record.count,
                    status,
                    score
                );
                ListItem::new(line)
            })
            .collect()
    };

    let topic_list = List::new(items).block(Block::default().borders(Borders::ALL).title("Review Topics"));

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
    frame.render_widget(topic_list, chunks[1]);
    frame.render_widget(help, chunks[2]);
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
    let topic = Paragraph::new(app.topic.clone())
        .style(topic_style)
        .block(Block::default().borders(Borders::ALL).title("Topic"));

    let count_style = if matches!(app.focused, InputField::Count) {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    let count = Paragraph::new(app.count_text.clone())
        .style(count_style)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Word Count (5-30)"),
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

    let example = Paragraph::new(vec![
        Line::from(Span::raw(format!("EN: {}", word.example_en))),
        Line::from(Span::raw(format!("KR: {}", word.example_ko))),
    ])
    .block(Block::default().borders(Borders::ALL).title("Example"))
    .wrap(Wrap { trim: true });

    let help = Paragraph::new(vec![
        Line::from("Enter: 다음 단어"),
        Line::from("Q: 바로 퀴즈 시작"),
        Line::from("Esc: 종료"),
    ])
    .block(Block::default().borders(Borders::ALL).title("Help"));

    frame.render_widget(title, chunks[0]);
    frame.render_widget(term, chunks[1]);
    frame.render_widget(meaning, chunks[2]);
    frame.render_widget(example, chunks[3]);
    frame.render_widget(help, chunks[4]);
}

fn draw_quiz(frame: &mut Frame<'_>, app: &App) {
    let Some(question) = app.current_quiz() else {
        return;
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
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
    .block(Block::default().borders(Borders::ALL).title("Quiz Mode"));

    let prompt = Paragraph::new(format!("'{}'의 뜻을 고르세요", question.word))
        .block(Block::default().borders(Borders::ALL).title("Question"));

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
            .title("Options (Up/Down + Enter)"),
    );

    frame.render_widget(title, chunks[0]);
    frame.render_widget(prompt, chunks[1]);
    frame.render_widget(list, chunks[2]);
}

fn draw_result(frame: &mut Frame<'_>, app: &App) {
    let popup = centered_rect(60, 30, frame.area());
    frame.render_widget(Clear, popup);

    let rate = if app.quiz_questions.is_empty() {
        0.0
    } else {
        (app.score as f64 / app.quiz_questions.len() as f64) * 100.0
    };

    let result = Paragraph::new(vec![
        Line::from(format!("정답: {}/{}", app.score, app.quiz_questions.len())),
        Line::from(format!("정답률: {:.1}%", rate)),
        Line::from(" "),
        Line::from("M: 메인으로 이동"),
        Line::from("S: 다시 학습하기"),
        Line::from("Q: 다시 시험보기"),
        Line::from("Esc: 종료"),
    ])
    .block(Block::default().borders(Borders::ALL).title("Result"))
    .wrap(Wrap { trim: true });

    frame.render_widget(result, popup);
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

fn build_quiz_questions(words: &[WordItem]) -> Vec<QuizQuestion> {
    let mut rng = rand::rng();
    let mut questions = Vec::with_capacity(words.len());

    for (index, word) in words.iter().enumerate() {
        let mut wrong_meanings: Vec<String> = words
            .iter()
            .enumerate()
            .filter(|(idx, _)| *idx != index)
            .map(|(_, item)| item.meaning_ko.clone())
            .collect();

        wrong_meanings.shuffle(&mut rng);
        let mut options = vec![word.meaning_ko.clone()];
        options.extend(wrong_meanings.into_iter().take(3));
        options.shuffle(&mut rng);

        let answer_index = options
            .iter()
            .position(|option| option == &word.meaning_ko)
            .unwrap_or(0);

        questions.push(QuizQuestion {
            word: word.term.clone(),
            options,
            answer_index,
        });
    }

    questions.shuffle(&mut rng);
    questions
}

async fn fetch_words(api_key: &str, topic: &str, count: usize) -> Result<Vec<WordItem>> {
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
                        "term": {"type": "string"},
                        "meaning_ko": {"type": "string"},
                        "example_en": {"type": "string"},
                        "example_ko": {"type": "string"}
                    },
                    "required": ["term", "meaning_ko", "example_en", "example_ko"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["words"],
        "additionalProperties": false
    });

    let user_prompt = format!(
        "Generate exactly {count} practical English vocabulary words for topic '{topic}'. Return Korean meanings and sentence examples."
    );

    let request_body = ChatCompletionRequest {
        model: "gpt-4o-mini",
        messages: vec![
            ChatMessage {
                role: "system",
                content: "You are a vocabulary generator for Korean learners. Provide CEFR A2-B2 words, avoid profanity.",
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
        temperature: 0.6,
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
