use std::collections::HashSet;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use rand::Rng;
use rand::seq::SliceRandom;
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
enum LearningLanguage {
    English,
    Japanese,
    Chinese,
}

impl LearningLanguage {
    const ALL: [Self; 3] = [Self::English, Self::Japanese, Self::Chinese];

    fn display_name_ko(self) -> &'static str {
        match self {
            Self::English => "영어",
            Self::Japanese => "일본어",
            Self::Chinese => "중국어",
        }
    }

    fn topic_language_name() -> &'static str {
        "Korean"
    }

    fn term_language_name(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::Japanese => "Japanese",
            Self::Chinese => "Chinese",
        }
    }

    fn rotate(self, delta: isize) -> Self {
        let len = Self::ALL.len() as isize;
        let idx = Self::ALL
            .iter()
            .position(|language| *language == self)
            .unwrap_or(0) as isize;
        let next = (idx + delta).rem_euclid(len) as usize;
        Self::ALL[next]
    }
}

fn default_learning_language() -> LearningLanguage {
    LearningLanguage::English
}

fn default_profile_name() -> String {
    env::var("HOSTNAME")
        .ok()
        .or_else(|| env::var("COMPUTERNAME").ok())
        .or_else(|| fs::read_to_string("/etc/hostname").ok())
        .or_else(|| env::var("USER").ok())
        .or_else(|| env::var("USERNAME").ok())
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty() && !name.eq_ignore_ascii_case("localhost"))
        .unwrap_or_else(|| "학습자".to_string())
}

#[derive(Debug, Deserialize)]
struct GenerationPayload {
    topic: String,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputField {
    ApiKey,
    ProfileName,
    LearningLanguage,
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
    #[serde(default)]
    skill_reward_stage: u8,
    #[serde(default)]
    study_completed_once: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct LanguageProgress {
    skill: u32,
    topic_history: Vec<TopicRecord>,
    selected_topic: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct LearningProgressStore {
    english: LanguageProgress,
    japanese: LanguageProgress,
    chinese: LanguageProgress,
}

impl LearningProgressStore {
    fn for_language(&self, language: LearningLanguage) -> &LanguageProgress {
        match language {
            LearningLanguage::English => &self.english,
            LearningLanguage::Japanese => &self.japanese,
            LearningLanguage::Chinese => &self.chinese,
        }
    }

    fn for_language_mut(&mut self, language: LearningLanguage) -> &mut LanguageProgress {
        match language {
            LearningLanguage::English => &mut self.english,
            LearningLanguage::Japanese => &mut self.japanese,
            LearningLanguage::Chinese => &mut self.chinese,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedStateV2 {
    version: u8,
    profile_name: String,
    total_xp: u32,
    english_skill: u32,
    topic_history: Vec<TopicRecord>,
    selected_topic: usize,
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedStateV3 {
    version: u8,
    profile_name: String,
    total_xp: u32,
    english_skill: u32,
    topic_history: Vec<TopicRecord>,
    selected_topic: usize,
    learning_language: LearningLanguage,
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedStateV4 {
    version: u8,
    profile_name: String,
    total_xp: u32,
    learning_language: LearningLanguage,
    progress: LearningProgressStore,
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedStateV5 {
    version: u8,
    profile_name: String,
    total_xp: u32,
    learning_language: LearningLanguage,
    progress: LearningProgressStore,
    api_key: String,
}

const STATE_VERSION: u8 = 5;
const STATE_DIR_NAME: &str = "jaturi";
const LEGACY_STATE_DIR_NAME: &str = "vocab_tui";
const STATE_FILE_NAME: &str = "state.bin";
const GENERATED_WORD_COUNT: usize = 10;

#[derive(Debug)]
struct App {
    screen: Screen,
    focused: InputField,
    api_key: String,
    learning_language: LearningLanguage,
    setup_editing: bool,
    progress: LearningProgressStore,
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
    message: String,
    pending_persist: bool,
    quit: bool,
}

impl Default for App {
    fn default() -> Self {
        let api_key = String::new();
        Self {
            screen: Screen::ApiKeySetup,
            focused: InputField::ApiKey,
            api_key,
            learning_language: default_learning_language(),
            setup_editing: false,
            progress: LearningProgressStore::default(),
            active_topic: None,
            words: Vec::new(),
            study_index: 0,
            quiz_questions: Vec::new(),
            quiz_reviews: Vec::new(),
            quiz_index: 0,
            selected_option: 0,
            typed_answer: String::new(),
            score: 0,
            profile_name: default_profile_name(),
            total_xp: 0,
            message: "API Key, 사용자 이름, 언어를 설정하고 Enter를 누르세요".to_string(),
            pending_persist: false,
            quit: false,
        }
    }
}

impl App {
    fn current_progress(&self) -> &LanguageProgress {
        self.progress.for_language(self.learning_language)
    }

    fn current_progress_mut(&mut self) -> &mut LanguageProgress {
        self.progress.for_language_mut(self.learning_language)
    }

    fn current_skill(&self) -> u32 {
        self.current_progress().skill
    }

    fn current_selected_topic(&self) -> usize {
        self.current_progress().selected_topic
    }

    fn add_xp(&mut self, xp: u32) {
        self.total_xp = self.total_xp.saturating_add(xp);
        self.pending_persist = true;
    }

    fn flush_pending_save(&mut self) -> bool {
        if !self.pending_persist {
            return true;
        }

        match self.save_persisted_state() {
            Ok(()) => {
                self.pending_persist = false;
                true
            }
            Err(err) => {
                self.screen = Screen::Error;
                self.message = format!(
                    "학습 상태 저장 실패: {err}. R: 다시 저장 시도 (저장 전까지 종료 불가)"
                );
                false
            }
        }
    }

    fn save_persisted_state(&self) -> Result<()> {
        let path = state_file_path()?;
        let state = PersistedStateV5 {
            version: STATE_VERSION,
            profile_name: self.profile_name.clone(),
            total_xp: self.total_xp,
            learning_language: self.learning_language,
            progress: self.progress.clone(),
            api_key: self.api_key.clone(),
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

        if let Ok((state, used)) = bincode::serde::decode_from_slice::<PersistedStateV5, _>(
            &bytes,
            bincode::config::standard(),
        ) {
            if used == bytes.len() && state.version == STATE_VERSION {
                self.profile_name = state.profile_name;
                self.total_xp = state.total_xp;
                self.progress = state.progress;
                self.learning_language = state.learning_language;
                self.api_key = state.api_key;
                self.sanitize_all_progresses();
                self.normalize_selection();
                return Ok(true);
            }
        }

        if let Ok((state, used)) = bincode::serde::decode_from_slice::<PersistedStateV4, _>(
            &bytes,
            bincode::config::standard(),
        ) {
            if used == bytes.len() && state.version == 4 {
                self.profile_name = state.profile_name;
                self.total_xp = state.total_xp;
                self.progress = state.progress;
                self.learning_language = state.learning_language;
                self.api_key.clear();
                self.sanitize_all_progresses();
                self.normalize_selection();
                return Ok(true);
            }
        }

        if let Ok((state, used)) = bincode::serde::decode_from_slice::<PersistedStateV3, _>(
            &bytes,
            bincode::config::standard(),
        ) {
            if used == bytes.len() && state.version == 3 {
                self.apply_legacy_single_progress(
                    state.profile_name,
                    state.total_xp,
                    state.english_skill,
                    state.topic_history,
                    state.selected_topic,
                    state.learning_language,
                );
                return Ok(true);
            }
        }

        let (legacy, used): (PersistedStateV2, usize) =
            bincode::serde::decode_from_slice(&bytes, bincode::config::standard())
                .context("학습 상태 파일 파싱 실패")?;
        if used != bytes.len() {
            bail!("학습 상태 파일 끝에 불필요한 데이터가 있습니다");
        }
        if legacy.version != 2 {
            bail!(
                "지원하지 않는 상태 버전입니다: {}, 기대값: 2, 3, 4 또는 {}",
                legacy.version,
                STATE_VERSION
            );
        }
        self.apply_legacy_single_progress(
            legacy.profile_name,
            legacy.total_xp,
            legacy.english_skill,
            legacy.topic_history,
            legacy.selected_topic,
            default_learning_language(),
        );

        Ok(true)
    }

    fn apply_legacy_single_progress(
        &mut self,
        profile_name: String,
        total_xp: u32,
        skill: u32,
        topic_history: Vec<TopicRecord>,
        selected_topic: usize,
        language: LearningLanguage,
    ) {
        self.profile_name = profile_name;
        self.total_xp = total_xp;
        self.progress = LearningProgressStore::default();
        let progress = self.progress.for_language_mut(language);
        progress.skill = skill;
        progress.topic_history = topic_history;
        if progress.topic_history.is_empty() {
            progress.selected_topic = 0;
        } else {
            progress.selected_topic = selected_topic.min(progress.topic_history.len() - 1);
        }
        self.learning_language = language;
        self.sanitize_all_progresses();
        self.normalize_selection();
    }

    fn sanitize_all_progresses(&mut self) {
        for language in LearningLanguage::ALL {
            let progress = self.progress.for_language_mut(language);
            progress.topic_history.retain(|record| {
                !record.words.is_empty()
                    && record
                        .words
                        .iter()
                        .all(|word| is_single_word_term(&word.term, language))
            });
            for record in &mut progress.topic_history {
                record.skill_reward_stage = record.skill_reward_stage.min(2);
            }
            if progress.topic_history.is_empty() {
                progress.selected_topic = 0;
            } else {
                progress.selected_topic = progress
                    .selected_topic
                    .min(progress.topic_history.len() - 1);
            }
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
        let normalized_name = self.profile_name.trim().to_string();
        if normalized_name.is_empty() {
            self.message = "사용자 이름을 입력해 주세요".to_string();
            self.focused = InputField::ProfileName;
            return;
        }
        self.api_key = normalized;
        self.profile_name = normalized_name;
        self.setup_editing = false;
        self.pending_persist = true;
        if self.flush_pending_save() {
            self.screen = Screen::Main;
            self.message = "N: 새 주제 생성, S: 학습, Q: 시험, K: 설정 수정".to_string();
        }
    }

    fn move_setup_focus(&mut self, delta: isize) {
        let fields = [
            InputField::ApiKey,
            InputField::ProfileName,
            InputField::LearningLanguage,
        ];
        let current = fields
            .iter()
            .position(|field| *field == self.focused)
            .unwrap_or(0) as isize;
        let next = (current + delta).rem_euclid(fields.len() as isize) as usize;
        self.focused = fields[next];
    }

    fn adjust_language(&mut self, delta: isize) {
        self.learning_language = self.learning_language.rotate(delta);
        self.normalize_selection();
    }

    fn handle_setup_input(&mut self, key: KeyEvent) {
        if self.setup_editing {
            match key.code {
                KeyCode::Enter => {
                    self.setup_editing = false;
                    self.message = "편집 완료. Up/Down으로 항목 이동, S로 설정 저장".to_string();
                }
                KeyCode::Left => {
                    if matches!(self.focused, InputField::LearningLanguage) {
                        self.adjust_language(-1);
                    }
                }
                KeyCode::Right => {
                    if matches!(self.focused, InputField::LearningLanguage) {
                        self.adjust_language(1);
                    }
                }
                KeyCode::Backspace => match self.focused {
                    InputField::ApiKey => {
                        self.api_key.pop();
                    }
                    InputField::ProfileName => {
                        self.profile_name.pop();
                    }
                    InputField::LearningLanguage => {}
                },
                KeyCode::Char(c) => {
                    if !key.modifiers.contains(KeyModifiers::CONTROL) {
                        match self.focused {
                            InputField::ApiKey => self.api_key.push(c),
                            InputField::ProfileName => {
                                if c != '\n' && c != '\r' {
                                    self.profile_name.push(c);
                                }
                            }
                            InputField::LearningLanguage => {
                                if c == 'h' || c == 'H' {
                                    self.adjust_language(-1);
                                } else if c == 'l' || c == 'L' {
                                    self.adjust_language(1);
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Up => self.move_setup_focus(-1),
            KeyCode::Down => self.move_setup_focus(1),
            KeyCode::Tab => self.move_setup_focus(1),
            KeyCode::BackTab => self.move_setup_focus(-1),
            KeyCode::Enter => {
                self.setup_editing = true;
                self.message = match self.focused {
                    InputField::ApiKey => "API Key 편집 중... Enter로 편집 종료".to_string(),
                    InputField::ProfileName => "이름 편집 중... Enter로 편집 종료".to_string(),
                    InputField::LearningLanguage => {
                        "언어 편집 중... Left/Right로 변경 후 Enter".to_string()
                    }
                };
            }
            KeyCode::Char('s') | KeyCode::Char('S') => {
                self.setup_api_key();
            }
            _ => {}
        }
    }

    fn topic_count(&self) -> usize {
        self.current_progress().topic_history.len()
    }

    fn move_topic_selection(&mut self, delta: isize) {
        let len = self.topic_count();
        let current_selected = self.current_selected_topic();
        let next = if len == 0 {
            0
        } else {
            let current = current_selected.min(len - 1) as isize;
            (current + delta).clamp(0, (len - 1) as isize) as usize
        };
        self.current_progress_mut().selected_topic = next;
    }

    fn normalize_selection_for_language(&mut self, language: LearningLanguage) {
        let progress = self.progress.for_language_mut(language);
        if progress.topic_history.is_empty() {
            progress.selected_topic = 0;
            return;
        }
        progress.selected_topic = progress
            .selected_topic
            .min(progress.topic_history.len() - 1);
    }

    fn normalize_selection(&mut self) {
        self.normalize_selection_for_language(self.learning_language);
        let has_topics = !self.current_progress().topic_history.is_empty();
        if !has_topics {
            self.active_topic = None;
        }
    }

    fn save_topic(&mut self, topic: String, words: Vec<WordItem>) -> Option<usize> {
        let index = {
            let progress = self.current_progress_mut();
            progress.topic_history.push(TopicRecord {
                topic,
                words,
                last_score: None,
                passed: false,
                skill_reward_stage: 0,
                study_completed_once: false,
            });
            let index = progress.topic_history.len() - 1;
            progress.selected_topic = index;
            index
        };
        self.pending_persist = true;
        if self.flush_pending_save() {
            Some(index)
        } else {
            None
        }
    }

    fn start_study_for(&mut self, index: usize) {
        if let Some(words) = self
            .current_progress()
            .topic_history
            .get(index)
            .map(|record| record.words.clone())
        {
            self.active_topic = Some(index);
            self.start_study(words);
        }
    }

    fn start_quiz_for(&mut self, index: usize) {
        if let Some(words) = self
            .current_progress()
            .topic_history
            .get(index)
            .map(|record| record.words.clone())
        {
            self.active_topic = Some(index);
            self.words = words;
            self.start_quiz();
        }
    }

    fn finish_quiz(&mut self) {
        if let Some(active) = self.active_topic {
            let total = self.quiz_questions.len();
            if total > 0 {
                let score = self.score;
                let passed = score * 100 >= total * 90;
                self.add_xp(5);
                let current_stage = self
                    .current_progress()
                    .topic_history
                    .get(active)
                    .map(|record| record.skill_reward_stage)
                    .unwrap_or(0)
                    .min(2);
                let target_stage: u8 = if score == total {
                    2
                } else if passed {
                    1
                } else {
                    0
                };
                let skill_gain = u32::from(target_stage.saturating_sub(current_stage));
                if skill_gain > 0 {
                    let progress = self.current_progress_mut();
                    progress.skill = progress.skill.saturating_add(skill_gain);
                }
                if let Some(record) = self.current_progress_mut().topic_history.get_mut(active) {
                    record.last_score = Some((score, total));
                    record.passed = record.passed || passed;
                    record.skill_reward_stage = record.skill_reward_stage.max(target_stage);
                }
                self.message = if skill_gain > 0 {
                    format!(
                        "복습 완료! +5 XP, {} 실력 +{}",
                        self.learning_language.display_name_ko(),
                        skill_gain
                    )
                } else {
                    "복습 완료! +5 XP".to_string()
                };
                self.pending_persist = true;
            }
        }
        self.screen = Screen::Result;
        self.flush_pending_save();
    }

    fn reward_study_completion(&mut self) {
        let xp_gain = if let Some(active_topic) = self.active_topic {
            let already_completed = self
                .current_progress()
                .topic_history
                .get(active_topic)
                .map(|record| record.study_completed_once)
                .unwrap_or(false);
            if let Some(record) = self
                .current_progress_mut()
                .topic_history
                .get_mut(active_topic)
            {
                record.study_completed_once = true;
            }
            if already_completed { 5 } else { 10 }
        } else {
            10
        };
        self.add_xp(xp_gain);
    }

    fn start_main(&mut self) {
        self.sanitize_all_progresses();
        self.normalize_selection();
        self.screen = Screen::Main;
        self.message = "N: 새 주제 생성, S: 학습, Q: 시험, K: 설정 수정, Enter: 학습".to_string();
    }

    fn selected_topic_record(&self) -> Option<&TopicRecord> {
        self.current_progress()
            .topic_history
            .get(self.current_selected_topic())
    }

    fn known_terms(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut terms = Vec::new();

        for record in &self.current_progress().topic_history {
            for word in &record.words {
                let key = normalize_term(&word.term);
                if !key.is_empty() && seen.insert(key) {
                    terms.push(word.term.clone());
                }
            }
        }

        terms
    }

    fn recent_topics(&self, limit: usize) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut topics = Vec::new();

        for record in self.current_progress().topic_history.iter().rev() {
            let topic = record.topic.trim();
            if topic.is_empty() {
                continue;
            }
            let normalized = normalize_topic(topic);
            if normalized.is_empty() || !seen.insert(normalized) {
                continue;
            }
            topics.push(topic.to_string());
            if topics.len() >= limit {
                break;
            }
        }

        topics
    }

    fn start_quiz(&mut self) {
        self.quiz_questions = build_quiz_questions(&self.words, self.learning_language);
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
                label: format!(
                    "{}: {}",
                    quiz_type_label(question.quiz_type),
                    question.target
                ),
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
            app.message = if app.api_key.trim().is_empty() {
                "이전 학습 기록을 불러왔습니다. N으로 새 주제 생성 시 API Key를 입력해 주세요"
                    .to_string()
            } else {
                "이전 학습 기록과 API Key를 불러왔습니다.".to_string()
            };
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
                        if let Some(index) = app.save_topic(output.topic, output.words) {
                            app.start_study_for(index);
                        }
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
            match event::read()? {
                Event::Key(key) => {
                    if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                        handle_key_event(key, &mut app, &tx).await;
                    }
                }
                Event::Paste(text) => {
                    handle_paste_event(&text, &mut app);
                }
                _ => {}
            }
        }
    }

    if app.pending_persist {
        app.save_persisted_state()
            .context("종료 전 학습 상태 저장 실패")?;
    }

    Ok(())
}

async fn handle_key_event(
    key: KeyEvent,
    app: &mut App,
    tx: &mpsc::UnboundedSender<Result<GenerationResult>>,
) {
    if key.code == KeyCode::Esc {
        if app.flush_pending_save() {
            app.quit = true;
        }
        return;
    }

    match app.screen {
        Screen::ApiKeySetup => app.handle_setup_input(key),
        Screen::Main => match key.code {
            KeyCode::Up => app.move_topic_selection(-1),
            KeyCode::Down => app.move_topic_selection(1),
            KeyCode::Enter | KeyCode::Char('s') | KeyCode::Char('S') => {
                if app.selected_topic_record().is_some() {
                    app.start_study_for(app.current_selected_topic());
                } else {
                    app.screen = Screen::TopicCreate;
                    app.message = format!(
                        "Enter: AI가 주제와 단어를 자동 생성합니다 ({}개)",
                        GENERATED_WORD_COUNT
                    );
                }
            }
            KeyCode::Char('q') | KeyCode::Char('Q') => {
                if app.selected_topic_record().is_some() {
                    app.start_quiz_for(app.current_selected_topic());
                }
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                app.screen = Screen::TopicCreate;
                app.message = format!(
                    "Enter: AI가 주제와 단어를 자동 생성합니다 ({}개)",
                    GENERATED_WORD_COUNT
                );
            }
            KeyCode::Char('k') | KeyCode::Char('K') => {
                app.screen = Screen::ApiKeySetup;
                app.focused = InputField::ApiKey;
                app.setup_editing = false;
                app.message = "Up/Down으로 항목 선택, Enter로 편집, S로 저장".to_string();
            }
            _ => {}
        },
        Screen::TopicCreate => match key.code {
            KeyCode::Enter => {
                if app.api_key.trim().is_empty() {
                    app.screen = Screen::ApiKeySetup;
                    app.focused = InputField::ApiKey;
                    app.setup_editing = false;
                    app.message = "API Key를 먼저 설정해 주세요".to_string();
                    return;
                }

                app.screen = Screen::Loading;
                app.message = "OpenAI에서 단어를 생성하는 중...".to_string();

                let api_key = app.api_key.clone();
                let excluded_terms = app.known_terms();
                let language_skill = app.current_skill();
                let current_level = level_progress_from_xp(app.total_xp).0;
                let recent_topics = app.recent_topics(10);
                let learning_language = app.learning_language;
                let tx = tx.clone();
                tokio::spawn(async move {
                    let result = fetch_words(
                        &api_key,
                        GENERATED_WORD_COUNT,
                        &excluded_terms,
                        language_skill,
                        current_level,
                        &recent_topics,
                        learning_language,
                    )
                    .await;
                    let _ = tx.send(result);
                });
            }
            KeyCode::Char('m') | KeyCode::Char('M') => app.start_main(),
            _ => {}
        },
        Screen::Loading => {}
        Screen::Study => match key.code {
            KeyCode::Enter => {
                if app.study_index + 1 < app.words.len() {
                    app.study_index += 1;
                } else {
                    app.reward_study_completion();
                    if app.flush_pending_save() {
                        app.start_quiz();
                    }
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
                if let Some(option_len) = choice_option_len
                    && app.selected_option + 1 < option_len
                {
                    app.selected_option += 1;
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
                    app.typed_answer.push(c);
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
                if app.pending_persist && !app.flush_pending_save() {
                    return;
                }
                if app.api_key.trim().is_empty() {
                    app.screen = Screen::ApiKeySetup;
                    app.focused = InputField::ApiKey;
                    app.setup_editing = false;
                } else {
                    app.start_main();
                }
            }
            KeyCode::Char('q') | KeyCode::Char('Q') => {
                if app.pending_persist {
                    app.message = "저장되지 않은 변경이 있습니다. R로 저장 재시도 후 종료해 주세요"
                        .to_string();
                } else {
                    app.quit = true;
                }
            }
            _ => {}
        },
    }
}

fn strip_newlines(text: &str) -> String {
    text.chars()
        .filter(|ch| *ch != '\n' && *ch != '\r')
        .collect()
}

fn handle_paste_event(text: &str, app: &mut App) {
    let cleaned = strip_newlines(text);
    if cleaned.is_empty() {
        return;
    }

    match app.screen {
        Screen::ApiKeySetup => {
            if !app.setup_editing {
                return;
            }
            match app.focused {
                InputField::ApiKey => app.api_key.push_str(&cleaned),
                InputField::ProfileName => app.profile_name.push_str(&cleaned),
                InputField::LearningLanguage => {}
            }
        }
        Screen::Quiz => {
            let is_text = matches!(
                app.current_quiz().map(|question| &question.answer),
                Some(QuizAnswer::Text(_))
            );
            if is_text {
                app.typed_answer.push_str(&cleaned);
            }
        }
        _ => {}
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
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(1),
        ])
        .margin(2)
        .split(frame.area());

    let title = Paragraph::new("jaturi")
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .block(Block::default().borders(Borders::ALL).title("초기 설정"));

    let api_value = if app.api_key.is_empty() {
        "(input your OpenAI API key)".to_string()
    } else {
        "*".repeat(app.api_key.len().min(40))
    };
    let api_style = if matches!(app.focused, InputField::ApiKey) && app.setup_editing {
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD)
    } else if matches!(app.focused, InputField::ApiKey) {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    let api = Paragraph::new(api_value)
        .style(api_style)
        .block(Block::default().borders(Borders::ALL).title("API Key"));

    let profile_style = if matches!(app.focused, InputField::ProfileName) && app.setup_editing {
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD)
    } else if matches!(app.focused, InputField::ProfileName) {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    let profile = Paragraph::new(app.profile_name.clone())
        .style(profile_style)
        .block(Block::default().borders(Borders::ALL).title("이름"));

    let language_style = if matches!(app.focused, InputField::LearningLanguage) && app.setup_editing
    {
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD)
    } else if matches!(app.focused, InputField::LearningLanguage) {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    let language = Paragraph::new(app.learning_language.display_name_ko())
        .style(language_style)
        .block(Block::default().borders(Borders::ALL).title("학습 언어"));

    let help = Paragraph::new(vec![
        Line::from("Up/Down: 항목 선택"),
        Line::from("Enter: 선택 항목 편집 시작/종료"),
        Line::from("S: 설정 저장 후 메인 이동"),
        Line::from("(언어 편집 중 Left/Right, H/L 사용 가능)"),
        Line::from("Esc: 종료"),
        Line::from(app.message.clone()),
    ])
    .block(Block::default().borders(Borders::ALL).title("Help"))
    .wrap(Wrap { trim: true });

    frame.render_widget(title, areas[0]);
    frame.render_widget(api, areas[1]);
    frame.render_widget(profile, areas[2]);
    frame.render_widget(language, areas[3]);
    frame.render_widget(help, areas[4]);
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

    let title = Paragraph::new("메인 메뉴")
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .block(Block::default().borders(Borders::ALL).title("jaturi"));

    let current_progress = app.current_progress();
    let selected_topic = app.current_selected_topic();

    let items: Vec<ListItem<'_>> = if current_progress.topic_history.is_empty() {
        vec![ListItem::new(
            "저장된 주제가 없습니다. N으로 새 주제를 생성하세요.",
        )]
    } else {
        current_progress
            .topic_history
            .iter()
            .enumerate()
            .map(|(idx, record)| {
                let status = if record.passed {
                    "passed"
                } else {
                    "in progress"
                };
                let score = record
                    .last_score
                    .map(|(value, total)| format!("  score: {value}/{total}"))
                    .unwrap_or_default();
                let line = format!(
                    "{}topic: {}  words: {}  status: {}{}",
                    if idx == selected_topic { "> " } else { "  " },
                    record.topic,
                    record.words.len(),
                    status,
                    score
                );
                ListItem::new(line)
            })
            .collect()
    };

    let topic_list = List::new(items).block(Block::default().borders(Borders::ALL).title(format!(
        "복습 주제 - {} 방",
        app.learning_language.display_name_ko()
    )));

    let (level, current_level_xp, current_level_required, next_level_remaining) =
        level_progress_from_xp(app.total_xp);

    let profile = Paragraph::new(vec![
        Line::from(format!("이름: {}", app.profile_name)),
        Line::from(format!(
            "학습 언어: {}",
            app.learning_language.display_name_ko()
        )),
        Line::from(format!(
            "전체 XP: {} (다음 레벨까지 {} XP)",
            app.total_xp, next_level_remaining
        )),
        Line::from(format!(
            "레벨: {} (레벨 진행 {}/{})",
            level, current_level_xp, current_level_required
        )),
        Line::from(format!(
            "언어 실력({}): {}",
            app.learning_language.display_name_ko(),
            app.current_skill()
        )),
    ])
    .block(Block::default().borders(Borders::ALL).title("프로필"))
    .wrap(Wrap { trim: true });

    let help = Paragraph::new(vec![
        Line::from("N: 새 주제 생성"),
        Line::from("Up/Down: 과거 주제 선택"),
        Line::from("K: 설정(API Key/이름/언어)"),
        Line::from("S/Enter: 단어 학습 시작, Q: 시험 보기"),
        Line::from("Esc: 종료"),
        Line::from(app.message.clone()),
    ])
    .block(Block::default().borders(Borders::ALL).title("동작"))
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

    let info = Paragraph::new(vec![
        Line::from("사용자 입력 없이 AI가 주제를 먼저 정합니다."),
        Line::from(format!(
            "주제 기반 단어를 항상 {}개 생성합니다.",
            GENERATED_WORD_COUNT
        )),
    ])
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title("Auto Generation"),
    )
    .wrap(Wrap { trim: true });

    let help = Paragraph::new(vec![
        Line::from("Enter: AI 생성 시작"),
        Line::from("M: 메인으로 이동"),
        Line::from("Esc: 종료"),
        Line::from(app.message.clone()),
    ])
    .block(Block::default().borders(Borders::ALL).title("Help"))
    .wrap(Wrap { trim: true });

    frame.render_widget(title, areas[0]);
    frame.render_widget(info, areas[1]);
    frame.render_widget(help, areas[2]);
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
        .block(Block::default().borders(Borders::ALL).title("뜻 (한국어)"));

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
    .block(Block::default().borders(Borders::ALL).title(format!(
        "Quiz Mode - {}",
        quiz_type_label(question.quiz_type)
    )));

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
                Line::from("문자 입력(일본어/중국어 IME, 붙여넣기 지원) + Backspace, Enter 제출"),
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

    let review = List::new(review_items).block(
        Block::default()
            .borders(Borders::ALL)
            .title("문제별 정오답"),
    );

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

fn build_quiz_questions(
    words: &[WordItem],
    learning_language: LearningLanguage,
) -> Vec<QuizQuestion> {
    let mut rng = rand::rng();
    let mut questions = Vec::with_capacity(words.len());

    for (index, word) in words.iter().enumerate() {
        let question_type = if matches!(
            learning_language,
            LearningLanguage::Japanese | LearningLanguage::Chinese
        ) {
            match rng.random_range(0..2) {
                0 => QuizType::MeaningChoice,
                _ => QuizType::FillBlankChoice,
            }
        } else {
            match rng.random_range(0..3) {
                0 => QuizType::MeaningChoice,
                1 => QuizType::FillBlankChoice,
                _ => QuizType::SpellingWrite,
            }
        };

        let question = match question_type {
            QuizType::MeaningChoice => {
                let mut wrong_meanings = unique_meanings(words, index);
                wrong_meanings.retain(|meaning| meaning.trim() != word.meaning_ko.trim());
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
                    prompt: format!("'{}'의 한국어 뜻을 고르세요", word.term),
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
                    "빈칸에 들어갈 단어를 고르세요\n한국어 뜻: {}\n철자 힌트: {}",
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
                    "뜻을 보고 단어를 직접 입력하세요\n한국어 뜻: {}",
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
    term.trim().to_lowercase()
}

fn normalize_topic(topic: &str) -> String {
    topic.trim().to_lowercase()
}

fn language_skill_band(language_skill: u32) -> &'static str {
    match language_skill {
        0..=2 => "beginner",
        3..=6 => "elementary",
        7..=11 => "intermediate",
        _ => "advanced",
    }
}

fn app_level_band(level: u32) -> &'static str {
    match level {
        1..=3 => "beginner",
        4..=7 => "intermediate",
        _ => "advanced",
    }
}

fn is_japanese_char(ch: char) -> bool {
    matches!(
        ch as u32,
        0x3040..=0x309F
            | 0x30A0..=0x30FF
            | 0x31F0..=0x31FF
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xF900..=0xFAFF
    )
}

fn is_chinese_char(ch: char) -> bool {
    matches!(
        ch as u32,
        0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xF900..=0xFAFF
    )
}

fn is_single_word_term(term: &str, language: LearningLanguage) -> bool {
    let trimmed = term.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_whitespace) {
        return false;
    }
    match language {
        LearningLanguage::English => trimmed
            .chars()
            .all(|ch| ch.is_ascii_alphabetic() || ch == '-' || ch == '\''),
        LearningLanguage::Japanese => {
            let chars: Vec<char> = trimmed.chars().collect();
            let has_japanese = chars.iter().copied().any(is_japanese_char);
            let has_ascii_alpha = chars.iter().any(|ch| ch.is_ascii_alphabetic());
            has_japanese && !has_ascii_alpha
        }
        LearningLanguage::Chinese => {
            let chars: Vec<char> = trimmed.chars().collect();
            let has_chinese = chars.iter().copied().any(is_chinese_char);
            let has_ascii_alpha = chars.iter().any(|ch| ch.is_ascii_alphabetic());
            let has_japanese_syllabary = chars.iter().any(|ch| {
                matches!(
                    *ch as u32,
                    0x3040..=0x309F
                        | 0x30A0..=0x30FF
                        | 0x31F0..=0x31FF
                )
            });
            has_chinese && !has_ascii_alpha && !has_japanese_syllabary
        }
    }
}

async fn fetch_words(
    api_key: &str,
    count: usize,
    excluded_terms: &[String],
    language_skill: u32,
    current_level: u32,
    recent_topics: &[String],
    learning_language: LearningLanguage,
) -> Result<GenerationResult> {
    let mut blocked = HashSet::new();
    for term in excluded_terms {
        let normalized = normalize_term(term);
        if !normalized.is_empty() {
            blocked.insert(normalized);
        }
    }

    let mut blocked_topics = HashSet::new();
    for topic in recent_topics {
        let normalized = normalize_topic(topic);
        if !normalized.is_empty() {
            blocked_topics.insert(normalized);
        }
    }

    let mut collected: Vec<WordItem> = Vec::with_capacity(count);
    let mut generated_topic: Option<String> = None;
    const MAX_ATTEMPTS: usize = 6;

    for _ in 0..MAX_ATTEMPTS {
        if collected.len() >= count {
            break;
        }

        let remaining = count - collected.len();
        let batch_count = remaining.saturating_add(4);
        let batch = fetch_words_batch(
            api_key,
            batch_count,
            language_skill,
            current_level,
            generated_topic.as_deref(),
            recent_topics,
            learning_language,
        )
        .await?;

        let topic = batch.topic.trim();
        let topic_key = normalize_topic(topic);

        if generated_topic.is_none() {
            if topic.is_empty() || blocked_topics.contains(&topic_key) {
                continue;
            }
            generated_topic = Some(topic.to_string());
        } else if let Some(active_topic) = &generated_topic {
            let active_key = normalize_topic(active_topic);
            if !topic_key.is_empty() && topic_key != active_key {
                continue;
            }
        }

        for item in batch.words {
            let key = normalize_term(&item.term);
            if key.is_empty()
                || blocked.contains(&key)
                || !is_single_word_term(&item.term, learning_language)
            {
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
        let used_topic = generated_topic.unwrap_or_else(|| "(주제 없음)".to_string());
        bail!(
            "중복 없는 새 단어를 충분히 만들지 못했습니다: 요청 {count}, 생성 {}. 최근 주제와 겹치지 않는 주제/단어가 부족할 수 있습니다. 생성된 주제: {used_topic}",
            collected.len(),
        );
    }

    Ok(GenerationResult {
        topic: generated_topic.unwrap_or_else(|| "AI 추천 주제".to_string()),
        words: collected,
    })
}

async fn fetch_words_batch(
    api_key: &str,
    count: usize,
    language_skill: u32,
    current_level: u32,
    fixed_topic: Option<&str>,
    recent_topics: &[String],
    learning_language: LearningLanguage,
) -> Result<GenerationPayload> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "topic": {
                "type": "string",
                "minLength": 1
            },
            "words": {
                "type": "array",
                "minItems": count,
                "maxItems": count,
                "items": {
                    "type": "object",
                    "properties": {
                        "term": {
                            "type": "string",
                            "minLength": 1
                        },
                        "meaning_ko": {"type": "string"}
                    },
                    "required": ["term", "meaning_ko"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["topic", "words"],
        "additionalProperties": false
    });

    let recent_topics_json =
        serde_json::to_string(recent_topics).context("최근 주제 직렬화 실패")?;
    let topic_instruction = if let Some(topic) = fixed_topic {
        format!(
            "Use this exact topic for `topic`: \"{}\". Keep generating new words within this same topic.",
            topic.trim()
        )
    } else {
        format!(
            "Choose one practical topic and write `topic` in {}. The topic must be semantically different from all recent topics.",
            LearningLanguage::topic_language_name()
        )
    };

    let user_prompt = format!(
        "Learner profile:\n- Learning language: {}\n- Language skill score: {} ({})\n- App level: {} ({})\n- Recent 10 topics to avoid: {}\n\n{}\nGenerate exactly {count} vocabulary items.\nEach `term` must be exactly one {} word and must not contain spaces.\n`meaning_ko` must be concise Korean meaning.\nDo not include words already used in beginner textbooks too frequently unless essential for the topic.",
        learning_language.term_language_name(),
        language_skill,
        language_skill_band(language_skill),
        current_level,
        app_level_band(current_level),
        recent_topics_json,
        topic_instruction,
        learning_language.term_language_name()
    );

    let system_prompt = format!(
        "You are a strict vocabulary generator for Korean learners. Return only JSON matching schema. `topic` must be in {}. Every `term` must be one {} word with no spaces. `meaning_ko` must be Korean. Match vocabulary difficulty to both language skill score and app level. Never choose a topic that overlaps with the provided recent topics.",
        LearningLanguage::topic_language_name(),
        learning_language.term_language_name(),
    );

    let request_body = ChatCompletionRequest {
        model: "gpt-4o-mini",
        messages: vec![
            ChatMessage {
                role: "system",
                content: &system_prompt,
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

    let payload: GenerationPayload =
        serde_json::from_str(&content).context("단어 JSON 파싱 실패")?;
    if payload.words.len() != count {
        bail!(
            "요청한 단어 수와 응답 단어 수가 다릅니다: 요청 {count}, 응답 {}",
            payload.words.len()
        );
    }
    Ok(payload)
}

fn state_file_path() -> Result<PathBuf> {
    let base_dir = dirs::data_local_dir()
        .or_else(dirs::data_dir)
        .ok_or_else(|| anyhow!("상태 저장 경로를 확인할 수 없습니다(OS 데이터 디렉터리 없음)"))?;

    let current_path = base_dir.join(STATE_DIR_NAME).join(STATE_FILE_NAME);
    if current_path.exists() {
        return Ok(current_path);
    }

    let legacy_path = base_dir.join(LEGACY_STATE_DIR_NAME).join(STATE_FILE_NAME);
    if !legacy_path.exists() {
        return Ok(current_path);
    }

    let legacy_bytes = fs::read(&legacy_path).context("이전 상태 파일 읽기 실패")?;
    atomic_write(&current_path, &legacy_bytes).context("이전 상태 파일 마이그레이션 실패")?;
    Ok(current_path)
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
