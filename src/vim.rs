use std::ops::Range;

use crate::{
    editor_surface::EditorSurface,
    keybindings::{Command, Scope},
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum VimMode {
    #[default]
    Normal,
    Insert,
    Replace,
    VisualCharacter,
    VisualLine,
}

impl VimMode {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Normal => "NORMAL",
            Self::Insert => "INSERT",
            Self::Replace => "REPLACE",
            Self::VisualCharacter | Self::VisualLine => "VISUAL",
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Register {
    pub text: String,
    pub linewise: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SearchDirection {
    #[default]
    Forward,
    Backward,
}

#[derive(Clone, Debug, Default)]
pub struct VimSession {
    pub register: Register,
    pub last_search: String,
    pub search_direction: SearchDirection,
    last_change: Option<RepeatChange>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VimRequest {
    Search {
        direction: SearchDirection,
        seed: Option<String>,
    },
    Ex,
    SystemPaste {
        before: bool,
    },
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct VimOutcome {
    pub changed: bool,
    pub request: Option<VimRequest>,
    pub copy_to_system: Option<String>,
}

#[derive(Clone, Copy)]
pub(crate) struct VimTextIndex<'a> {
    line_starts: &'a [usize],
    line_byte_starts: &'a [usize],
    character_len: usize,
}

impl<'a> VimTextIndex<'a> {
    pub(crate) fn new(
        line_starts: &'a [usize],
        line_byte_starts: &'a [usize],
        character_len: usize,
    ) -> Self {
        Self {
            line_starts,
            line_byte_starts,
            character_len,
        }
    }

    fn line(&self, character: usize) -> usize {
        self.line_starts
            .partition_point(|start| *start <= character.min(self.character_len))
            .saturating_sub(1)
    }

    fn line_start(&self, character: usize) -> usize {
        self.line_starts[self.line(character)]
    }

    fn line_end(&self, character: usize) -> usize {
        self.line_starts
            .get(self.line(character) + 1)
            .map_or(self.character_len, |start| start.saturating_sub(1))
    }

    fn byte_index(&self, text: &str, character: usize) -> usize {
        let character = character.min(self.character_len);
        let line = self.line(character);
        let byte_start = self.line_byte_starts[line];
        text[byte_start..]
            .char_indices()
            .nth(character - self.line_starts[line])
            .map_or(text.len(), |(offset, _)| byte_start + offset)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operator {
    Delete,
    Change,
    Yank,
    Indent,
    Outdent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FindDirection {
    Forward,
    Backward,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Find {
    direction: FindDirection,
    till: bool,
    character: char,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Awaiting {
    Find {
        direction: FindDirection,
        till: bool,
    },
    Replace,
    TextObject {
        around: bool,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Motion {
    Left,
    Right,
    Up,
    Down,
    LineStart,
    FirstNonBlank,
    LineEnd,
    FileStart,
    FileEnd,
    WordForward(bool),
    WordEnd(bool),
    WordBack(bool),
    ParagraphBack,
    ParagraphForward,
    Find(Find),
}

#[derive(Clone, Copy, Debug)]
enum RepeatTarget {
    Motion(Motion),
    TextObject { character: char, around: bool },
    Line,
}

#[derive(Clone, Debug)]
enum RepeatChange {
    Direct {
        command: Command,
        count: usize,
        character: Option<char>,
    },
    Operator {
        operator: Operator,
        target: RepeatTarget,
        count: usize,
    },
    Change {
        target: RepeatTarget,
        count: usize,
        text: String,
    },
    Insert {
        command: Command,
        text: String,
    },
}

#[derive(Clone, Debug)]
pub struct VimState {
    mode: VimMode,
    count: usize,
    operator: Option<Operator>,
    operator_count: usize,
    awaiting: Option<Awaiting>,
    last_find: Option<Find>,
    visual_anchor: Option<usize>,
    use_system_register: bool,
    insert_command: Option<Command>,
    insert_text: String,
    insert_target: Option<(RepeatTarget, usize)>,
    preferred_column: Option<usize>,
}

impl Default for VimState {
    fn default() -> Self {
        Self {
            mode: VimMode::Normal,
            count: 0,
            operator: None,
            operator_count: 1,
            awaiting: None,
            last_find: None,
            visual_anchor: None,
            use_system_register: false,
            insert_command: None,
            insert_text: String::new(),
            insert_target: None,
            preferred_column: None,
        }
    }
}

impl VimState {
    pub const fn mode(&self) -> VimMode {
        self.mode
    }

    pub const fn scope(&self) -> Scope {
        if self.operator.is_some() {
            Scope::VimOperator
        } else {
            match self.mode {
                VimMode::Normal => Scope::VimNormal,
                VimMode::Insert => Scope::VimInsert,
                VimMode::Replace => Scope::VimReplace,
                VimMode::VisualCharacter | VimMode::VisualLine => Scope::VimVisual,
            }
        }
    }

    pub fn status(&self) -> String {
        let mut prefix = String::new();
        if self.count > 0 {
            prefix.push_str(&self.count.to_string());
        }
        if let Some(operator) = self.operator {
            prefix.push(match operator {
                Operator::Delete => 'd',
                Operator::Change => 'c',
                Operator::Yank => 'y',
                Operator::Indent => '>',
                Operator::Outdent => '<',
            });
        }
        if !prefix.is_empty() {
            prefix
        } else {
            self.mode.label().to_owned()
        }
    }

    pub const fn text_input_enabled(&self) -> bool {
        matches!(self.mode, VimMode::Insert | VimMode::Replace)
    }

    pub fn replacing(&self) -> bool {
        self.mode == VimMode::Replace
    }

    pub const fn awaits_character(&self) -> bool {
        self.awaiting.is_some()
    }

    pub fn record_insert_text(&mut self, text: &str) {
        if self.text_input_enabled() {
            self.insert_text.push_str(text);
        }
    }

    pub fn clear_preferred_column(&mut self) {
        self.preferred_column = None;
    }

    pub fn sync_pointer_selection(&mut self, editor: &EditorSurface, linewise: bool) {
        self.preferred_column = None;
        if self.text_input_enabled() {
            return;
        }
        self.cancel_pending();
        let selection = editor.selection();
        if selection.is_empty() {
            self.mode = VimMode::Normal;
            self.visual_anchor = None;
        } else {
            self.mode = if linewise {
                VimMode::VisualLine
            } else {
                VimMode::VisualCharacter
            };
            self.visual_anchor = Some(selection.start);
        }
    }

    pub fn cancel_pending(&mut self) {
        self.count = 0;
        self.operator = None;
        self.operator_count = 1;
        self.awaiting = None;
        self.use_system_register = false;
    }

    pub fn execute(
        &mut self,
        command: Command,
        editor: &mut EditorSurface,
        text: &mut String,
        session: &mut VimSession,
    ) -> VimOutcome {
        if let Some(digit) = command_digit(command) {
            if digit == 0 && self.count == 0 {
                return self.execute_motion(Motion::LineStart, editor, text, session);
            }
            self.count = self.count.saturating_mul(10).saturating_add(digit);
            return VimOutcome::default();
        }
        if command == Command::VimNormal {
            return self.enter_normal(editor, text, session);
        }
        if let Some(motion) = command_motion(command, self.last_find) {
            return self.execute_motion(motion, editor, text, session);
        }
        self.preferred_column = None;
        match command {
            Command::VimInsert => self.enter_insert(Command::VimInsert, editor),
            Command::VimInsertLineStart => {
                let cursor = first_nonblank(text, editor.cursor());
                editor.set_selection(cursor, cursor);
                self.enter_insert(command, editor)
            }
            Command::VimAppend => {
                let cursor = (editor.cursor() + 1).min(line_end(text, editor.cursor()));
                editor.set_selection(cursor, cursor);
                self.enter_insert(command, editor)
            }
            Command::VimAppendLineEnd => {
                let cursor = line_end(text, editor.cursor());
                editor.set_selection(cursor, cursor);
                self.enter_insert(command, editor)
            }
            Command::VimOpenBelow => {
                let end = line_end(text, editor.cursor());
                editor.set_selection(end, end);
                editor.begin_transaction();
                let changed = editor.replace_selection(text, "\n");
                self.enter_insert(command, editor).with_changed(changed)
            }
            Command::VimOpenAbove => {
                let start = line_start(text, editor.cursor());
                editor.set_selection(start, start);
                editor.begin_transaction();
                let changed = editor.replace_selection(text, "\n");
                editor.set_selection(start, start);
                self.enter_insert(command, editor).with_changed(changed)
            }
            Command::VimSubstituteChar => {
                let end =
                    (editor.cursor() + self.take_count()).min(line_end(text, editor.cursor()));
                editor.set_selection(editor.cursor(), end);
                editor.begin_transaction();
                let changed = editor.replace_selection(text, "");
                self.enter_insert(command, editor).with_changed(changed)
            }
            Command::VimSubstituteLine => {
                let count = self.take_count();
                let range = line_range(text, editor.cursor(), count);
                self.operator = Some(Operator::Change);
                self.apply_operator_range(
                    range,
                    true,
                    editor,
                    text,
                    session,
                    Some((Operator::Change, RepeatTarget::Line, count)),
                )
            }
            Command::VimChangeLineEnd => {
                self.direct_operator_to_line_end(Operator::Change, editor, text, session)
            }
            Command::VimReplaceMode => {
                self.mode = VimMode::Replace;
                self.insert_command = Some(command);
                self.insert_text.clear();
                editor.begin_transaction();
                VimOutcome::default()
            }
            Command::VimVisualCharacter => {
                self.toggle_visual(VimMode::VisualCharacter, editor, text)
            }
            Command::VimVisualLine => self.toggle_visual(VimMode::VisualLine, editor, text),
            Command::VimOperatorDelete => self.operator(Operator::Delete, editor, text, session),
            Command::VimOperatorChange => self.operator(Operator::Change, editor, text, session),
            Command::VimOperatorYank => self.operator(Operator::Yank, editor, text, session),
            Command::VimOperatorIndent => self.operator(Operator::Indent, editor, text, session),
            Command::VimOperatorOutdent => self.operator(Operator::Outdent, editor, text, session),
            Command::VimTextInner => {
                self.awaiting = Some(Awaiting::TextObject { around: false });
                VimOutcome::default()
            }
            Command::VimTextAround => {
                self.awaiting = Some(Awaiting::TextObject { around: true });
                VimOutcome::default()
            }
            Command::VimFindForward => {
                self.awaiting = Some(Awaiting::Find {
                    direction: FindDirection::Forward,
                    till: false,
                });
                VimOutcome::default()
            }
            Command::VimFindBackward => {
                self.awaiting = Some(Awaiting::Find {
                    direction: FindDirection::Backward,
                    till: false,
                });
                VimOutcome::default()
            }
            Command::VimTillForward => {
                self.awaiting = Some(Awaiting::Find {
                    direction: FindDirection::Forward,
                    till: true,
                });
                VimOutcome::default()
            }
            Command::VimTillBackward => {
                self.awaiting = Some(Awaiting::Find {
                    direction: FindDirection::Backward,
                    till: true,
                });
                VimOutcome::default()
            }
            Command::VimReplaceCharacter => {
                self.awaiting = Some(Awaiting::Replace);
                VimOutcome::default()
            }
            Command::VimDeleteCharacter => {
                self.delete_characters(false, command, editor, text, session)
            }
            Command::VimDeleteCharacterLeft => {
                self.delete_characters(true, command, editor, text, session)
            }
            Command::VimDeleteLineEnd => {
                self.direct_operator_to_line_end(Operator::Delete, editor, text, session)
            }
            Command::VimJoinLines => self.join_lines(command, editor, text, session),
            Command::VimToggleCase => self.toggle_case(command, editor, text, session),
            Command::VimPutAfter => self.put(false, command, editor, text, session),
            Command::VimPutBefore => self.put(true, command, editor, text, session),
            Command::VimUndo => VimOutcome {
                changed: editor.undo(text),
                ..VimOutcome::default()
            },
            Command::VimRedo => VimOutcome {
                changed: editor.redo(text),
                ..VimOutcome::default()
            },
            Command::VimRepeat => self.repeat(editor, text, session),
            Command::VimSearchForward => VimOutcome {
                request: Some(VimRequest::Search {
                    direction: SearchDirection::Forward,
                    seed: None,
                }),
                ..VimOutcome::default()
            },
            Command::VimSearchBackward => VimOutcome {
                request: Some(VimRequest::Search {
                    direction: SearchDirection::Backward,
                    seed: None,
                }),
                ..VimOutcome::default()
            },
            Command::VimSearchNext => VimOutcome {
                request: Some(VimRequest::Search {
                    direction: session.search_direction,
                    seed: Some(session.last_search.clone()),
                }),
                ..VimOutcome::default()
            },
            Command::VimSearchPrevious => VimOutcome {
                request: Some(VimRequest::Search {
                    direction: reverse_search(session.search_direction),
                    seed: Some(session.last_search.clone()),
                }),
                ..VimOutcome::default()
            },
            Command::VimSearchWordForward | Command::VimSearchWordBackward => {
                let seed = word_under_cursor(text, editor.cursor());
                VimOutcome {
                    request: Some(VimRequest::Search {
                        direction: if command == Command::VimSearchWordForward {
                            SearchDirection::Forward
                        } else {
                            SearchDirection::Backward
                        },
                        seed,
                    }),
                    ..VimOutcome::default()
                }
            }
            Command::VimRegisterSystem => {
                self.use_system_register = true;
                VimOutcome::default()
            }
            Command::VimEx => VimOutcome {
                request: Some(VimRequest::Ex),
                ..VimOutcome::default()
            },
            _ => VimOutcome::default(),
        }
    }

    pub(crate) fn execute_indexed(
        &mut self,
        command: Command,
        editor: &mut EditorSurface,
        text: &mut String,
        session: &mut VimSession,
        index: VimTextIndex<'_>,
    ) -> VimOutcome {
        if let Some(digit) = command_digit(command) {
            if digit == 0 && self.count == 0 {
                return self.execute_motion_with_index(
                    Motion::LineStart,
                    editor,
                    text,
                    session,
                    Some(index),
                );
            }
            self.count = self.count.saturating_mul(10).saturating_add(digit);
            return VimOutcome::default();
        }
        if let Some(motion) = command_motion(command, self.last_find) {
            return self.execute_motion_with_index(motion, editor, text, session, Some(index));
        }
        self.execute(command, editor, text, session)
    }

    pub(crate) fn provide_character_indexed(
        &mut self,
        character: char,
        editor: &mut EditorSurface,
        text: &mut String,
        session: &mut VimSession,
        index: VimTextIndex<'_>,
    ) -> VimOutcome {
        self.provide_character_with_index(character, editor, text, session, Some(index))
    }

    pub fn provide_character(
        &mut self,
        character: char,
        editor: &mut EditorSurface,
        text: &mut String,
        session: &mut VimSession,
    ) -> VimOutcome {
        self.provide_character_with_index(character, editor, text, session, None)
    }

    fn provide_character_with_index(
        &mut self,
        character: char,
        editor: &mut EditorSurface,
        text: &mut String,
        session: &mut VimSession,
        index: Option<VimTextIndex<'_>>,
    ) -> VimOutcome {
        let Some(awaiting) = self.awaiting.take() else {
            return VimOutcome::default();
        };
        match awaiting {
            Awaiting::Find { direction, till } => {
                let find = Find {
                    direction,
                    till,
                    character,
                };
                self.last_find = Some(find);
                self.execute_motion_with_index(Motion::Find(find), editor, text, session, index)
            }
            Awaiting::Replace => {
                let count = self.take_count();
                let start = editor.cursor();
                let end = (start + count).min(
                    index.map_or_else(|| line_end(text, start), |index| index.line_end(start)),
                );
                editor.set_selection(start, end);
                let replacement =
                    std::iter::repeat_n(character, end.saturating_sub(start)).collect::<String>();
                let changed = editor.replace_selection(text, &replacement);
                let last = index.map_or_else(
                    || text.chars().count().saturating_sub(1),
                    |index| index.character_len.saturating_sub(1),
                );
                editor.set_selection(start.min(last), start.min(last));
                if changed {
                    session.last_change = Some(RepeatChange::Direct {
                        command: Command::VimReplaceCharacter,
                        count,
                        character: Some(character),
                    });
                }
                VimOutcome {
                    changed,
                    ..VimOutcome::default()
                }
            }
            Awaiting::TextObject { around } => {
                let range = index.map_or_else(
                    || text_object(text, editor.cursor(), character, around),
                    |index| indexed_text_object(text, editor.cursor(), character, around, index),
                );
                let Some(range) = range else {
                    self.cancel_pending();
                    return VimOutcome::default();
                };
                let count = self.operator_count.saturating_mul(self.take_count());
                self.apply_operator_range_with_index(
                    range,
                    false,
                    editor,
                    text,
                    session,
                    Some((
                        self.operator.expect("text objects require an operator"),
                        RepeatTarget::TextObject { character, around },
                        count,
                    )),
                    index,
                )
            }
        }
    }

    pub fn insert_text(
        &mut self,
        value: &str,
        editor: &mut EditorSurface,
        text: &mut String,
    ) -> bool {
        if !self.text_input_enabled() || value.is_empty() {
            return false;
        }
        self.record_insert_text(value);
        if self.replacing() {
            let count = value.chars().count();
            let end = (editor.cursor() + count).min(line_end(text, editor.cursor()));
            editor.set_selection(editor.cursor(), end);
        }
        editor.replace_selection(text, value)
    }

    pub fn paste_system_text(
        &mut self,
        before: bool,
        value: &str,
        editor: &mut EditorSurface,
        text: &mut String,
        session: &mut VimSession,
    ) -> bool {
        session.register = Register {
            text: value.to_owned(),
            linewise: value.ends_with('\n'),
        };
        self.use_system_register = false;
        self.put(
            before,
            if before {
                Command::VimPutBefore
            } else {
                Command::VimPutAfter
            },
            editor,
            text,
            session,
        )
        .changed
    }

    fn enter_insert(&mut self, command: Command, editor: &mut EditorSurface) -> VimOutcome {
        self.mode = VimMode::Insert;
        self.insert_command = Some(command);
        self.insert_text.clear();
        self.cancel_pending();
        editor.begin_transaction();
        VimOutcome::default()
    }

    fn enter_normal(
        &mut self,
        editor: &mut EditorSurface,
        text: &str,
        session: &mut VimSession,
    ) -> VimOutcome {
        let was_insert = self.text_input_enabled();
        editor.end_transaction();
        if was_insert {
            let inserted = std::mem::take(&mut self.insert_text);
            if let Some((target, count)) = self.insert_target.take() {
                session.last_change = Some(RepeatChange::Change {
                    target,
                    count,
                    text: inserted,
                });
            } else if !inserted.is_empty() {
                session.last_change = Some(RepeatChange::Insert {
                    command: self.insert_command.unwrap_or(Command::VimInsert),
                    text: inserted,
                });
            }
        }
        if was_insert {
            let start = line_start(text, editor.cursor());
            if editor.cursor() > start {
                editor.set_selection(editor.cursor() - 1, editor.cursor() - 1);
            }
        }
        self.mode = VimMode::Normal;
        self.visual_anchor = None;
        self.insert_command = None;
        self.cancel_pending();
        clamp_normal(editor, text);
        VimOutcome::default()
    }

    fn take_count(&mut self) -> usize {
        let count = self.count.max(1);
        self.count = 0;
        count
    }

    fn operator(
        &mut self,
        operator: Operator,
        editor: &mut EditorSurface,
        text: &mut String,
        session: &mut VimSession,
    ) -> VimOutcome {
        if matches!(self.mode, VimMode::VisualCharacter | VimMode::VisualLine) {
            let linewise = self.mode == VimMode::VisualLine;
            let range = editor.selection();
            self.operator = Some(operator);
            return self.apply_operator_range(range, linewise, editor, text, session, None);
        }
        if self.operator == Some(operator) {
            let count = self.operator_count.saturating_mul(self.take_count());
            let range = line_range(text, editor.cursor(), count);
            return self.apply_operator_range(
                range,
                true,
                editor,
                text,
                session,
                Some((operator, RepeatTarget::Line, count)),
            );
        }
        self.operator = Some(operator);
        self.operator_count = self.take_count();
        VimOutcome::default()
    }

    fn execute_motion(
        &mut self,
        motion: Motion,
        editor: &mut EditorSurface,
        text: &mut String,
        session: &mut VimSession,
    ) -> VimOutcome {
        self.execute_motion_with_index(motion, editor, text, session, None)
    }

    fn execute_motion_with_index(
        &mut self,
        motion: Motion,
        editor: &mut EditorSurface,
        text: &mut String,
        session: &mut VimSession,
        index: Option<VimTextIndex<'_>>,
    ) -> VimOutcome {
        let count = self
            .take_count()
            .saturating_mul(if self.operator.is_some() {
                self.operator_count
            } else {
                1
            });
        if let Some(operator) = self.operator {
            let (range, linewise) = index.map_or_else(
                || motion_range(text, editor.cursor(), motion, count),
                |index| indexed_motion_range(text, editor.cursor(), motion, count, index),
            );
            return self.apply_operator_range_with_index(
                range,
                linewise,
                editor,
                text,
                session,
                Some((operator, RepeatTarget::Motion(motion), count)),
                index,
            );
        }
        let destination = if matches!(motion, Motion::Up | Motion::Down) {
            let column = *self.preferred_column.get_or_insert_with(|| {
                editor.cursor().saturating_sub(index.map_or_else(
                    || line_start(text, editor.cursor()),
                    |index| index.line_start(editor.cursor()),
                ))
            });
            index.map_or_else(
                || {
                    vertical_destination_with_column(
                        text,
                        editor.cursor(),
                        if motion == Motion::Up { -1 } else { 1 },
                        count,
                        column,
                    )
                },
                |index| {
                    indexed_vertical_destination(
                        editor.cursor(),
                        if motion == Motion::Up { -1 } else { 1 },
                        count,
                        column,
                        index,
                    )
                },
            )
        } else {
            self.preferred_column = None;
            index.map_or_else(
                || motion_destination(text, editor.cursor(), motion, count),
                |index| indexed_motion_destination(text, editor.cursor(), motion, count, index),
            )
        };
        match self.mode {
            VimMode::VisualCharacter => {
                let anchor = self.visual_anchor.unwrap_or(editor.cursor());
                editor.set_selection(anchor, destination);
            }
            VimMode::VisualLine => {
                let anchor = self.visual_anchor.unwrap_or(editor.cursor());
                editor.set_selection(
                    index.map_or_else(
                        || line_start(text, anchor),
                        |index| index.line_start(anchor),
                    ),
                    index.map_or_else(
                        || line_end_with_newline(text, destination),
                        |index| {
                            let end = index.line_end(destination);
                            (end + usize::from(end < index.character_len)).min(index.character_len)
                        },
                    ),
                );
            }
            _ => editor.set_selection(destination, destination),
        }
        if let Some(index) = index {
            indexed_clamp_normal(editor, index);
        } else {
            clamp_normal(editor, text);
        }
        VimOutcome::default()
    }

    fn apply_operator_range(
        &mut self,
        range: Range<usize>,
        linewise: bool,
        editor: &mut EditorSurface,
        text: &mut String,
        session: &mut VimSession,
        repeat: Option<(Operator, RepeatTarget, usize)>,
    ) -> VimOutcome {
        self.apply_operator_range_with_index(range, linewise, editor, text, session, repeat, None)
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_operator_range_with_index(
        &mut self,
        range: Range<usize>,
        linewise: bool,
        editor: &mut EditorSurface,
        text: &mut String,
        session: &mut VimSession,
        repeat: Option<(Operator, RepeatTarget, usize)>,
        index: Option<VimTextIndex<'_>>,
    ) -> VimOutcome {
        let Some(operator) = self
            .operator
            .take()
            .or_else(|| repeat.map(|repeat| repeat.0))
        else {
            return VimOutcome::default();
        };
        self.operator_count = 1;
        self.count = 0;
        let character_len = index.map_or_else(|| text.chars().count(), |index| index.character_len);
        let range = range.start.min(character_len)..range.end.min(character_len);
        let selected = index.map_or_else(
            || char_slice(text, range.clone()).to_owned(),
            |index| {
                text[index.byte_index(text, range.start)..index.byte_index(text, range.end)]
                    .to_owned()
            },
        );
        let mut outcome = VimOutcome::default();
        if matches!(
            operator,
            Operator::Delete | Operator::Change | Operator::Yank
        ) {
            session.register = Register {
                text: selected.clone(),
                linewise,
            };
            if self.use_system_register {
                outcome.copy_to_system = Some(selected.clone());
            }
        }
        match operator {
            Operator::Yank => {
                editor.set_selection(range.start, range.start);
            }
            Operator::Delete | Operator::Change => {
                editor.set_selection(range.start, range.end);
                if operator == Operator::Change {
                    editor.begin_transaction();
                }
                let replacement =
                    if operator == Operator::Change && linewise && selected.ends_with('\n') {
                        "\n"
                    } else {
                        ""
                    };
                outcome.changed = editor.replace_selection(text, replacement);
                let new_len = character_len - range.len() + replacement.chars().count();
                let cursor = if operator == Operator::Change {
                    range.start.min(new_len)
                } else {
                    range.start.min(new_len.saturating_sub(1))
                };
                editor.set_selection(cursor, cursor);
                if operator == Operator::Change {
                    self.enter_insert(Command::VimOperatorChange, editor);
                }
            }
            Operator::Indent | Operator::Outdent => {
                let replacement = indent_text(&selected, operator == Operator::Indent);
                editor.set_selection(range.start, range.end);
                outcome.changed = editor.replace_selection(text, &replacement);
                editor.set_selection(range.start, range.start);
            }
        }
        if outcome.changed
            && let Some((operator, target, count)) = repeat
        {
            if operator == Operator::Change {
                self.insert_target = Some((target, count));
            } else {
                session.last_change = Some(RepeatChange::Operator {
                    operator,
                    target,
                    count,
                });
            }
        }
        self.mode = if operator == Operator::Change {
            VimMode::Insert
        } else {
            VimMode::Normal
        };
        self.visual_anchor = None;
        self.use_system_register = false;
        if operator != Operator::Change {
            if let Some(index) = index {
                indexed_clamp_normal(editor, index);
            } else {
                clamp_normal(editor, text);
            }
        }
        outcome
    }

    fn toggle_visual(
        &mut self,
        mode: VimMode,
        editor: &mut EditorSurface,
        text: &str,
    ) -> VimOutcome {
        if self.mode == mode {
            self.mode = VimMode::Normal;
            self.visual_anchor = None;
            editor.set_selection(editor.cursor(), editor.cursor());
        } else {
            self.mode = mode;
            let anchor = self.visual_anchor.unwrap_or(editor.cursor());
            self.visual_anchor = Some(anchor);
            if mode == VimMode::VisualLine {
                editor.set_selection(
                    line_start(text, anchor),
                    line_end_with_newline(text, editor.cursor()),
                );
            } else {
                editor.set_selection(anchor, (editor.cursor() + 1).min(text.chars().count()));
            }
        }
        VimOutcome::default()
    }

    fn delete_characters(
        &mut self,
        left: bool,
        command: Command,
        editor: &mut EditorSurface,
        text: &mut String,
        session: &mut VimSession,
    ) -> VimOutcome {
        let count = self.take_count();
        let cursor = editor.cursor();
        let range = if left {
            cursor.saturating_sub(count)..cursor
        } else {
            cursor..(cursor + count).min(line_end(text, cursor))
        };
        self.operator = Some(Operator::Delete);
        let outcome = self.apply_operator_range(range, false, editor, text, session, None);
        if outcome.changed {
            session.last_change = Some(RepeatChange::Direct {
                command,
                count,
                character: None,
            });
        }
        outcome
    }

    fn direct_operator_to_line_end(
        &mut self,
        operator: Operator,
        editor: &mut EditorSurface,
        text: &mut String,
        session: &mut VimSession,
    ) -> VimOutcome {
        self.operator = Some(operator);
        let count = self.take_count();
        let range = editor.cursor()..line_end(text, editor.cursor());
        self.apply_operator_range(
            range,
            false,
            editor,
            text,
            session,
            Some((operator, RepeatTarget::Motion(Motion::LineEnd), count)),
        )
    }

    fn join_lines(
        &mut self,
        command: Command,
        editor: &mut EditorSurface,
        text: &mut String,
        session: &mut VimSession,
    ) -> VimOutcome {
        let count = self.take_count().max(2);
        let start = editor.cursor();
        let mut changed = false;
        editor.begin_transaction();
        for _ in 1..count {
            let end = line_end(text, start);
            if end >= text.chars().count() {
                break;
            }
            let next = text
                .chars()
                .skip(end + 1)
                .take_while(|character| character.is_whitespace() && *character != '\n')
                .count();
            editor.set_selection(end, end + 1 + next);
            changed |= editor.replace_selection(text, " ");
        }
        editor.end_transaction();
        editor.set_selection(
            start.min(text.chars().count().saturating_sub(1)),
            start.min(text.chars().count().saturating_sub(1)),
        );
        if changed {
            session.last_change = Some(RepeatChange::Direct {
                command,
                count,
                character: None,
            });
        }
        VimOutcome {
            changed,
            ..VimOutcome::default()
        }
    }

    fn toggle_case(
        &mut self,
        command: Command,
        editor: &mut EditorSurface,
        text: &mut String,
        session: &mut VimSession,
    ) -> VimOutcome {
        let count = self.take_count();
        let range = if matches!(self.mode, VimMode::VisualCharacter | VimMode::VisualLine) {
            editor.selection()
        } else {
            editor.cursor()..(editor.cursor() + count).min(line_end(text, editor.cursor()))
        };
        let replacement = char_slice(text, range.clone())
            .chars()
            .flat_map(|character| {
                if character.is_lowercase() {
                    character.to_uppercase().collect::<Vec<_>>()
                } else {
                    character.to_lowercase().collect::<Vec<_>>()
                }
            })
            .collect::<String>();
        editor.set_selection(range.start, range.end);
        let changed = editor.replace_selection(text, &replacement);
        editor.set_selection(
            range.start.min(text.chars().count().saturating_sub(1)),
            range.start.min(text.chars().count().saturating_sub(1)),
        );
        self.mode = VimMode::Normal;
        self.visual_anchor = None;
        if changed {
            session.last_change = Some(RepeatChange::Direct {
                command,
                count,
                character: None,
            });
        }
        VimOutcome {
            changed,
            ..VimOutcome::default()
        }
    }

    fn put(
        &mut self,
        before: bool,
        command: Command,
        editor: &mut EditorSurface,
        text: &mut String,
        session: &mut VimSession,
    ) -> VimOutcome {
        if self.use_system_register {
            return VimOutcome {
                request: Some(VimRequest::SystemPaste { before }),
                ..VimOutcome::default()
            };
        }
        let count = self.take_count();
        let replacement = session.register.text.repeat(count);
        if replacement.is_empty() {
            return VimOutcome::default();
        }
        let cursor = if session.register.linewise {
            if before {
                line_start(text, editor.cursor())
            } else {
                line_end_with_newline(text, editor.cursor())
            }
        } else if before {
            editor.cursor()
        } else {
            (editor.cursor() + 1).min(text.chars().count())
        };
        editor.set_selection(cursor, cursor);
        let changed = editor.replace_selection(text, &replacement);
        editor.set_selection(
            cursor.min(text.chars().count().saturating_sub(1)),
            cursor.min(text.chars().count().saturating_sub(1)),
        );
        if changed {
            session.last_change = Some(RepeatChange::Direct {
                command,
                count,
                character: None,
            });
        }
        VimOutcome {
            changed,
            ..VimOutcome::default()
        }
    }

    fn repeat(
        &mut self,
        editor: &mut EditorSurface,
        text: &mut String,
        session: &mut VimSession,
    ) -> VimOutcome {
        let Some(change) = session.last_change.clone() else {
            return VimOutcome::default();
        };
        match change {
            RepeatChange::Direct {
                command,
                count,
                character,
            } => {
                self.count = count;
                if command == Command::VimReplaceCharacter {
                    self.awaiting = Some(Awaiting::Replace);
                    return self.provide_character(character.unwrap_or(' '), editor, text, session);
                }
                self.execute(command, editor, text, session)
            }
            RepeatChange::Operator {
                operator,
                target,
                count,
            } => {
                self.operator = Some(operator);
                self.operator_count = 1;
                self.count = count;
                self.repeat_target(target, editor, text, session)
            }
            RepeatChange::Change {
                target,
                count,
                text: inserted,
            } => {
                self.operator = Some(Operator::Change);
                self.operator_count = 1;
                self.count = count;
                let mut outcome = self.repeat_target(target, editor, text, session);
                outcome.changed |= self.insert_text(&inserted, editor, text);
                outcome.changed |= self.enter_normal(editor, text, session).changed;
                outcome
            }
            RepeatChange::Insert {
                command,
                text: inserted,
            } => {
                let mut outcome = self.execute(command, editor, text, session);
                outcome.changed |= self.insert_text(&inserted, editor, text);
                let normal = self.enter_normal(editor, text, session);
                outcome.changed |= normal.changed;
                outcome
            }
        }
    }

    fn repeat_target(
        &mut self,
        target: RepeatTarget,
        editor: &mut EditorSurface,
        text: &mut String,
        session: &mut VimSession,
    ) -> VimOutcome {
        match target {
            RepeatTarget::Motion(motion) => self.execute_motion(motion, editor, text, session),
            RepeatTarget::TextObject { character, around } => {
                let Some(range) = text_object(text, editor.cursor(), character, around) else {
                    self.cancel_pending();
                    return VimOutcome::default();
                };
                let count = self.take_count();
                self.apply_operator_range(
                    range,
                    false,
                    editor,
                    text,
                    session,
                    Some((
                        self.operator.expect("repeat target requires an operator"),
                        target,
                        count,
                    )),
                )
            }
            RepeatTarget::Line => {
                let count = self.take_count();
                let range = line_range(text, editor.cursor(), count);
                self.apply_operator_range(
                    range,
                    true,
                    editor,
                    text,
                    session,
                    Some((
                        self.operator.expect("repeat target requires an operator"),
                        target,
                        count,
                    )),
                )
            }
        }
    }
}

impl VimOutcome {
    fn with_changed(mut self, changed: bool) -> Self {
        self.changed |= changed;
        self
    }
}

fn command_digit(command: Command) -> Option<usize> {
    Some(match command {
        Command::VimCount0 => 0,
        Command::VimCount1 => 1,
        Command::VimCount2 => 2,
        Command::VimCount3 => 3,
        Command::VimCount4 => 4,
        Command::VimCount5 => 5,
        Command::VimCount6 => 6,
        Command::VimCount7 => 7,
        Command::VimCount8 => 8,
        Command::VimCount9 => 9,
        _ => return None,
    })
}

fn command_motion(command: Command, last_find: Option<Find>) -> Option<Motion> {
    Some(match command {
        Command::VimMoveLeft => Motion::Left,
        Command::VimMoveRight => Motion::Right,
        Command::VimMoveUp => Motion::Up,
        Command::VimMoveDown => Motion::Down,
        Command::VimLineStart => Motion::LineStart,
        Command::VimFirstNonBlank => Motion::FirstNonBlank,
        Command::VimLineEnd => Motion::LineEnd,
        Command::VimFileStart => Motion::FileStart,
        Command::VimFileEnd => Motion::FileEnd,
        Command::VimWordForward => Motion::WordForward(false),
        Command::VimBigWordForward => Motion::WordForward(true),
        Command::VimWordEnd => Motion::WordEnd(false),
        Command::VimBigWordEnd => Motion::WordEnd(true),
        Command::VimWordBack => Motion::WordBack(false),
        Command::VimBigWordBack => Motion::WordBack(true),
        Command::VimParagraphBack => Motion::ParagraphBack,
        Command::VimParagraphForward => Motion::ParagraphForward,
        Command::VimRepeatFind => Motion::Find(last_find?),
        Command::VimReverseFind => {
            let mut find = last_find?;
            find.direction = match find.direction {
                FindDirection::Forward => FindDirection::Backward,
                FindDirection::Backward => FindDirection::Forward,
            };
            Motion::Find(find)
        }
        _ => return None,
    })
}

fn motion_range(text: &str, cursor: usize, motion: Motion, count: usize) -> (Range<usize>, bool) {
    if matches!(motion, Motion::Up | Motion::Down) {
        let destination = motion_destination(text, cursor, motion, count);
        let start = line_start(text, cursor.min(destination));
        let end = line_end_with_newline(text, cursor.max(destination));
        return (start..end, true);
    }
    let destination = motion_destination_unclamped(text, cursor, motion, count);
    let inclusive = matches!(
        motion,
        Motion::WordEnd(_)
            | Motion::LineEnd
            | Motion::FileEnd
            | Motion::Find(Find { till: false, .. })
    );
    if destination >= cursor {
        (
            cursor..(destination + usize::from(inclusive)).min(text.chars().count()),
            false,
        )
    } else {
        (
            destination..(cursor + usize::from(inclusive)).min(text.chars().count()),
            false,
        )
    }
}

fn indexed_motion_range(
    text: &str,
    cursor: usize,
    motion: Motion,
    count: usize,
    index: VimTextIndex<'_>,
) -> (Range<usize>, bool) {
    if matches!(motion, Motion::Up | Motion::Down) {
        let destination = indexed_motion_destination(text, cursor, motion, count, index);
        let start = index.line_start(cursor.min(destination));
        let end = indexed_line_end_with_newline(cursor.max(destination), index);
        return (start..end, true);
    }
    let destination = indexed_motion_destination_unclamped(text, cursor, motion, count, index);
    let inclusive = matches!(
        motion,
        Motion::WordEnd(_)
            | Motion::LineEnd
            | Motion::FileEnd
            | Motion::Find(Find { till: false, .. })
    );
    if destination >= cursor {
        (
            cursor..(destination + usize::from(inclusive)).min(index.character_len),
            false,
        )
    } else {
        (
            destination..(cursor + usize::from(inclusive)).min(index.character_len),
            false,
        )
    }
}

fn indexed_motion_destination(
    text: &str,
    cursor: usize,
    motion: Motion,
    count: usize,
    index: VimTextIndex<'_>,
) -> usize {
    indexed_motion_destination_unclamped(text, cursor, motion, count, index).min(
        index
            .character_len
            .saturating_sub(usize::from(index.character_len > 0)),
    )
}

fn indexed_motion_destination_unclamped(
    text: &str,
    cursor: usize,
    motion: Motion,
    count: usize,
    index: VimTextIndex<'_>,
) -> usize {
    let mut destination = cursor.min(index.character_len);
    for _ in 0..count.max(1) {
        destination = match motion {
            Motion::Left => destination
                .saturating_sub(1)
                .max(index.line_start(destination)),
            Motion::Right => (destination + 1).min(index.line_end(destination).saturating_sub(1)),
            Motion::Up => indexed_vertical_destination(
                destination,
                -1,
                1,
                destination.saturating_sub(index.line_start(destination)),
                index,
            ),
            Motion::Down => indexed_vertical_destination(
                destination,
                1,
                1,
                destination.saturating_sub(index.line_start(destination)),
                index,
            ),
            Motion::LineStart => index.line_start(destination),
            Motion::FirstNonBlank => indexed_first_nonblank(text, destination, index),
            Motion::LineEnd => index
                .line_end(destination)
                .saturating_sub(1)
                .max(index.line_start(destination)),
            Motion::FileStart => 0,
            Motion::FileEnd => index.character_len.saturating_sub(1),
            Motion::WordForward(big) => indexed_word_forward(text, destination, big, index),
            Motion::WordEnd(big) => indexed_word_end(text, destination, big, index),
            Motion::WordBack(big) => indexed_word_back(text, destination, big, index),
            Motion::ParagraphBack => indexed_paragraph_back(text, destination, index),
            Motion::ParagraphForward => indexed_paragraph_forward(text, destination, index),
            Motion::Find(find) => {
                indexed_find_character(text, destination, find, index).unwrap_or(destination)
            }
        };
    }
    destination
}

fn indexed_line_end_with_newline(character: usize, index: VimTextIndex<'_>) -> usize {
    let end = index.line_end(character);
    (end + usize::from(end < index.character_len)).min(index.character_len)
}

fn indexed_paragraph_back(text: &str, cursor: usize, index: VimTextIndex<'_>) -> usize {
    let mut line = index.line(cursor);
    while line > 0 {
        line -= 1;
        if indexed_line_is_blank(text, line, index) {
            return index.line_starts[line];
        }
    }
    0
}

fn indexed_paragraph_forward(text: &str, cursor: usize, index: VimTextIndex<'_>) -> usize {
    let mut line = index.line(cursor) + 1;
    while line < index.line_starts.len() {
        if indexed_line_is_blank(text, line, index) {
            return index.line_starts[line];
        }
        line += 1;
    }
    index
        .character_len
        .saturating_sub(usize::from(index.character_len > 0))
}

fn indexed_line_is_blank(text: &str, line: usize, index: VimTextIndex<'_>) -> bool {
    let start = index.line_byte_starts[line];
    let end = index
        .line_byte_starts
        .get(line + 1)
        .map_or(text.len(), |next| next.saturating_sub(1));
    text[start..end].trim().is_empty()
}

fn indexed_first_nonblank(text: &str, cursor: usize, index: VimTextIndex<'_>) -> usize {
    let start = index.line_start(cursor);
    start
        + text[index.byte_index(text, start)..]
            .chars()
            .take_while(|character| *character == ' ' || *character == '\t')
            .count()
}

fn indexed_vertical_destination(
    cursor: usize,
    direction: i8,
    count: usize,
    column: usize,
    index: VimTextIndex<'_>,
) -> usize {
    let mut line = index.line(cursor);
    for _ in 0..count.max(1) {
        if direction < 0 {
            if line == 0 {
                break;
            }
            line -= 1;
        } else if line + 1 < index.line_starts.len() {
            line += 1;
        } else {
            break;
        }
    }
    let start = index.line_starts[line];
    let end = index.line_end(start);
    start
        + column.min(
            end.saturating_sub(start)
                .saturating_sub(usize::from(end > start)),
        )
}

fn indexed_word_forward(text: &str, cursor: usize, big: bool, index: VimTextIndex<'_>) -> usize {
    let mut destination = cursor.min(index.character_len);
    let mut characters = text[index.byte_index(text, destination)..]
        .chars()
        .peekable();
    if let Some(character) = characters.next() {
        destination += 1;
        let class = word_class(character, big);
        while characters
            .next_if(|character| word_class(*character, big) == class)
            .is_some()
        {
            destination += 1;
        }
    }
    while characters
        .next_if(|character| character.is_whitespace())
        .is_some()
    {
        destination += 1;
    }
    destination
}

fn indexed_word_end(text: &str, cursor: usize, big: bool, index: VimTextIndex<'_>) -> usize {
    let mut destination = (cursor + 1).min(index.character_len);
    let mut characters = text[index.byte_index(text, destination)..].chars();
    let Some(mut character) = characters.next() else {
        return index.character_len.saturating_sub(1);
    };
    while character.is_whitespace() {
        destination += 1;
        let Some(next) = characters.next() else {
            return index.character_len.saturating_sub(1);
        };
        character = next;
    }
    let class = word_class(character, big);
    for character in characters {
        if word_class(character, big) != class {
            break;
        }
        destination += 1;
    }
    destination
}

fn indexed_word_back(text: &str, cursor: usize, big: bool, index: VimTextIndex<'_>) -> usize {
    let mut destination = cursor.min(index.character_len);
    let mut characters = text[..index.byte_index(text, destination)].chars().rev();
    let Some(mut character) = characters.next() else {
        return 0;
    };
    while character.is_whitespace() {
        destination = destination.saturating_sub(1);
        let Some(next) = characters.next() else {
            return 0;
        };
        character = next;
    }
    destination = destination.saturating_sub(1);
    let class = word_class(character, big);
    for character in characters {
        if word_class(character, big) != class {
            break;
        }
        destination = destination.saturating_sub(1);
    }
    destination
}

fn indexed_find_character(
    text: &str,
    cursor: usize,
    find: Find,
    index: VimTextIndex<'_>,
) -> Option<usize> {
    let start = index.line_start(cursor);
    let end = index.line_end(cursor);
    match find.direction {
        FindDirection::Forward => {
            let from = cursor.saturating_add(1).min(end);
            text[index.byte_index(text, from)..index.byte_index(text, end)]
                .chars()
                .position(|character| character == find.character)
                .map(|offset| from + offset)
                .map(|found| found.saturating_sub(usize::from(find.till)))
        }
        FindDirection::Backward => text
            [index.byte_index(text, start)..index.byte_index(text, cursor)]
            .chars()
            .rev()
            .position(|character| character == find.character)
            .map(|offset| cursor - 1 - offset)
            .map(|found| (found + usize::from(find.till)).min(end.saturating_sub(1))),
    }
}

fn motion_destination(text: &str, cursor: usize, motion: Motion, count: usize) -> usize {
    motion_destination_unclamped(text, cursor, motion, count).min(
        text.chars()
            .count()
            .saturating_sub(usize::from(!text.is_empty())),
    )
}

fn motion_destination_unclamped(text: &str, cursor: usize, motion: Motion, count: usize) -> usize {
    let mut destination = cursor.min(text.chars().count());
    for _ in 0..count.max(1) {
        destination = match motion {
            Motion::Left => destination
                .saturating_sub(1)
                .max(line_start(text, destination)),
            Motion::Right => (destination + 1).min(line_end(text, destination).saturating_sub(1)),
            Motion::Up => vertical_destination(text, destination, -1),
            Motion::Down => vertical_destination(text, destination, 1),
            Motion::LineStart => line_start(text, destination),
            Motion::FirstNonBlank => first_nonblank(text, destination),
            Motion::LineEnd => line_end(text, destination)
                .saturating_sub(1)
                .max(line_start(text, destination)),
            Motion::FileStart => 0,
            Motion::FileEnd => text.chars().count().saturating_sub(1),
            Motion::WordForward(big) => word_forward(text, destination, big),
            Motion::WordEnd(big) => word_end(text, destination, big),
            Motion::WordBack(big) => word_back(text, destination, big),
            Motion::ParagraphBack => paragraph_back(text, destination),
            Motion::ParagraphForward => paragraph_forward(text, destination),
            Motion::Find(find) => find_character(text, destination, find).unwrap_or(destination),
        };
    }
    destination
}

fn line_start(text: &str, cursor: usize) -> usize {
    text.chars()
        .take(cursor)
        .collect::<Vec<_>>()
        .iter()
        .rposition(|character| *character == '\n')
        .map_or(0, |index| index + 1)
}

fn line_end(text: &str, cursor: usize) -> usize {
    let chars = text.chars().collect::<Vec<_>>();
    let cursor = cursor.min(chars.len());
    chars[cursor..]
        .iter()
        .position(|character| *character == '\n')
        .map_or(chars.len(), |offset| cursor + offset)
}

fn line_end_with_newline(text: &str, cursor: usize) -> usize {
    let end = line_end(text, cursor);
    (end + usize::from(end < text.chars().count())).min(text.chars().count())
}

fn line_range(text: &str, cursor: usize, count: usize) -> Range<usize> {
    let start = line_start(text, cursor);
    let mut end = start;
    for _ in 0..count.max(1) {
        end = line_end_with_newline(text, end);
    }
    start..end
}

fn first_nonblank(text: &str, cursor: usize) -> usize {
    let start = line_start(text, cursor);
    start
        + text
            .chars()
            .skip(start)
            .take_while(|character| *character == ' ' || *character == '\t')
            .count()
}

fn vertical_destination(text: &str, cursor: usize, direction: i8) -> usize {
    let start = line_start(text, cursor);
    let column = cursor.saturating_sub(start);
    if direction < 0 {
        if start == 0 {
            return cursor;
        }
        let previous_end = start - 1;
        let previous_start = line_start(text, previous_end);
        previous_start
            + column.min(
                previous_end
                    .saturating_sub(previous_start)
                    .saturating_sub(usize::from(previous_end > previous_start)),
            )
    } else {
        let end = line_end(text, cursor);
        if end >= text.chars().count() {
            return cursor;
        }
        let next_start = end + 1;
        let next_end = line_end(text, next_start);
        next_start
            + column.min(
                next_end
                    .saturating_sub(next_start)
                    .saturating_sub(usize::from(next_end > next_start)),
            )
    }
}

fn vertical_destination_with_column(
    text: &str,
    cursor: usize,
    direction: i8,
    count: usize,
    column: usize,
) -> usize {
    let mut destination = cursor;
    for _ in 0..count.max(1) {
        let start = line_start(text, destination);
        let target_start = if direction < 0 {
            if start == 0 {
                break;
            }
            line_start(text, start - 1)
        } else {
            let end = line_end(text, destination);
            if end >= text.chars().count() {
                break;
            }
            end + 1
        };
        let target_end = line_end(text, target_start);
        destination = target_start
            + column.min(
                target_end
                    .saturating_sub(target_start)
                    .saturating_sub(usize::from(target_end > target_start)),
            );
    }
    destination
}

fn word_class(character: char, big: bool) -> u8 {
    if character.is_whitespace() {
        0
    } else if big || character.is_alphanumeric() || character == '_' {
        1
    } else {
        2
    }
}

fn word_forward(text: &str, cursor: usize, big: bool) -> usize {
    let chars = text.chars().collect::<Vec<_>>();
    let mut index = cursor.min(chars.len());
    if index < chars.len() {
        let class = word_class(chars[index], big);
        while index < chars.len() && word_class(chars[index], big) == class {
            index += 1;
        }
    }
    while index < chars.len() && chars[index].is_whitespace() {
        index += 1;
    }
    index
}

fn word_end(text: &str, cursor: usize, big: bool) -> usize {
    let chars = text.chars().collect::<Vec<_>>();
    let mut index = (cursor + 1).min(chars.len());
    while index < chars.len() && chars[index].is_whitespace() {
        index += 1;
    }
    if index >= chars.len() {
        return chars.len().saturating_sub(1);
    }
    let class = word_class(chars[index], big);
    while index + 1 < chars.len() && word_class(chars[index + 1], big) == class {
        index += 1;
    }
    index
}

fn word_back(text: &str, cursor: usize, big: bool) -> usize {
    let chars = text.chars().collect::<Vec<_>>();
    let mut index = cursor.min(chars.len());
    while index > 0 && chars[index - 1].is_whitespace() {
        index -= 1;
    }
    if index == 0 {
        return 0;
    }
    let class = word_class(chars[index - 1], big);
    while index > 0 && word_class(chars[index - 1], big) == class {
        index -= 1;
    }
    index
}

fn paragraph_back(text: &str, cursor: usize) -> usize {
    let mut index = line_start(text, cursor);
    while index > 0 {
        let previous = line_start(text, index - 1);
        if char_slice(text, previous..index.saturating_sub(1))
            .trim()
            .is_empty()
        {
            return previous;
        }
        index = previous;
    }
    0
}

fn paragraph_forward(text: &str, cursor: usize) -> usize {
    let mut index = line_end_with_newline(text, cursor);
    let len = text.chars().count();
    while index < len {
        let end = line_end(text, index);
        if char_slice(text, index..end).trim().is_empty() {
            return index;
        }
        index = line_end_with_newline(text, index);
    }
    len.saturating_sub(usize::from(len > 0))
}

fn find_character(text: &str, cursor: usize, find: Find) -> Option<usize> {
    let start = line_start(text, cursor);
    let end = line_end(text, cursor);
    let chars = text.chars().collect::<Vec<_>>();
    match find.direction {
        FindDirection::Forward => chars[cursor.saturating_add(1).min(end)..end]
            .iter()
            .position(|character| *character == find.character)
            .map(|offset| cursor + 1 + offset)
            .map(|index| index.saturating_sub(usize::from(find.till))),
        FindDirection::Backward => chars[start..cursor.min(chars.len())]
            .iter()
            .rposition(|character| *character == find.character)
            .map(|offset| start + offset)
            .map(|index| (index + usize::from(find.till)).min(end.saturating_sub(1))),
    }
}

fn text_object(text: &str, cursor: usize, character: char, around: bool) -> Option<Range<usize>> {
    match character {
        'w' => word_object(text, cursor, false, around),
        'W' => word_object(text, cursor, true, around),
        '"' | '\'' | '`' => quote_object(text, cursor, character, around),
        '(' | ')' | 'b' => pair_object(text, cursor, '(', ')', around),
        '[' | ']' => pair_object(text, cursor, '[', ']', around),
        '{' | '}' | 'B' => pair_object(text, cursor, '{', '}', around),
        'p' => Some(paragraph_object(text, cursor, around)),
        _ => None,
    }
}

fn indexed_text_object(
    text: &str,
    cursor: usize,
    character: char,
    around: bool,
    index: VimTextIndex<'_>,
) -> Option<Range<usize>> {
    match character {
        'w' => indexed_word_object(text, cursor, false, around, index),
        'W' => indexed_word_object(text, cursor, true, around, index),
        '"' | '\'' | '`' => indexed_quote_object(text, cursor, character, around, index),
        '(' | ')' | 'b' => indexed_pair_object(text, cursor, '(', ')', around, index),
        '[' | ']' => indexed_pair_object(text, cursor, '[', ']', around, index),
        '{' | '}' | 'B' => indexed_pair_object(text, cursor, '{', '}', around, index),
        'p' => Some(indexed_paragraph_object(text, cursor, around, index)),
        _ => None,
    }
}

fn indexed_word_object(
    text: &str,
    cursor: usize,
    big: bool,
    around: bool,
    index: VimTextIndex<'_>,
) -> Option<Range<usize>> {
    if index.character_len == 0 {
        return None;
    }
    let mut cursor = cursor.min(index.character_len - 1);
    let mut byte = index.byte_index(text, cursor);
    while let Some(character) = text[byte..].chars().next()
        && character.is_whitespace()
    {
        cursor += 1;
        byte += character.len_utf8();
    }
    let character = text[byte..].chars().next()?;
    let class = word_class(character, big);
    let mut start = cursor;
    for character in text[..byte].chars().rev() {
        if word_class(character, big) != class {
            break;
        }
        start -= 1;
    }
    let mut end = cursor;
    for character in text[byte..].chars() {
        if word_class(character, big) != class {
            break;
        }
        end += 1;
    }
    if around {
        let original_end = end;
        for character in text[index.byte_index(text, end)..].chars() {
            if !character.is_whitespace() {
                break;
            }
            end += 1;
        }
        if end == original_end {
            for character in text[..index.byte_index(text, start)].chars().rev() {
                if !character.is_whitespace() {
                    break;
                }
                start -= 1;
            }
        }
    }
    Some(start..end)
}

fn indexed_quote_object(
    text: &str,
    cursor: usize,
    quote: char,
    around: bool,
    index: VimTextIndex<'_>,
) -> Option<Range<usize>> {
    let start = index.line_start(cursor);
    let end = index.line_end(cursor);
    let left_end = (cursor + 1).min(end);
    let left_offset = text[index.byte_index(text, start)..index.byte_index(text, left_end)]
        .chars()
        .rev()
        .position(|character| character == quote)?;
    let left = left_end - 1 - left_offset;
    let right_start = cursor + usize::from(left == cursor);
    let right = right_start
        + text[index.byte_index(text, right_start.min(end))..index.byte_index(text, end)]
            .chars()
            .position(|character| character == quote)?;
    Some(if around {
        left..right + 1
    } else {
        left + 1..right
    })
}

fn indexed_pair_object(
    text: &str,
    cursor: usize,
    open: char,
    close: char,
    around: bool,
    index: VimTextIndex<'_>,
) -> Option<Range<usize>> {
    let mut stack = Vec::new();
    let through_cursor = (cursor + 1).min(index.character_len);
    for (position, character) in text[..index.byte_index(text, through_cursor)]
        .chars()
        .enumerate()
    {
        if character == open {
            stack.push(position);
        } else if character == close {
            stack.pop();
        }
    }
    let left = stack.pop()?;
    let mut depth = 0;
    let right = text[index.byte_index(text, left + 1)..]
        .chars()
        .enumerate()
        .find_map(|(offset, character)| {
            if character == open {
                depth += 1;
            } else if character == close {
                if depth == 0 {
                    return Some(left + 1 + offset);
                }
                depth -= 1;
            }
            None
        })?;
    Some(if around {
        left..right + 1
    } else {
        left + 1..right
    })
}

fn indexed_paragraph_object(
    text: &str,
    cursor: usize,
    around: bool,
    index: VimTextIndex<'_>,
) -> Range<usize> {
    let start = indexed_paragraph_back(text, cursor, index);
    let mut end = indexed_paragraph_forward(text, cursor, index);
    if around {
        end = indexed_line_end_with_newline(end, index);
    }
    start..end
}

fn word_object(text: &str, cursor: usize, big: bool, around: bool) -> Option<Range<usize>> {
    let chars = text.chars().collect::<Vec<_>>();
    if chars.is_empty() {
        return None;
    }
    let mut cursor = cursor.min(chars.len() - 1);
    while cursor < chars.len() && chars[cursor].is_whitespace() {
        cursor += 1;
    }
    if cursor >= chars.len() {
        return None;
    }
    let class = word_class(chars[cursor], big);
    let mut start = cursor;
    let mut end = cursor + 1;
    while start > 0 && word_class(chars[start - 1], big) == class {
        start -= 1;
    }
    while end < chars.len() && word_class(chars[end], big) == class {
        end += 1;
    }
    if around {
        let original = end;
        while end < chars.len() && chars[end].is_whitespace() {
            end += 1;
        }
        if end == original {
            while start > 0 && chars[start - 1].is_whitespace() {
                start -= 1;
            }
        }
    }
    Some(start..end)
}

fn quote_object(text: &str, cursor: usize, quote: char, around: bool) -> Option<Range<usize>> {
    let start = line_start(text, cursor);
    let end = line_end(text, cursor);
    let chars = text.chars().collect::<Vec<_>>();
    let left = start
        + chars[start..(cursor + 1).min(chars.len())]
            .iter()
            .rposition(|character| *character == quote)?;
    let right_start = cursor + usize::from(left == cursor);
    let right = right_start
        + chars[right_start.min(end)..end]
            .iter()
            .position(|character| *character == quote)?;
    Some(if around {
        left..right + 1
    } else {
        left + 1..right
    })
}

fn pair_object(
    text: &str,
    cursor: usize,
    open: char,
    close: char,
    around: bool,
) -> Option<Range<usize>> {
    let chars = text.chars().collect::<Vec<_>>();
    let mut stack = Vec::new();
    for (index, character) in chars
        .iter()
        .copied()
        .enumerate()
        .take((cursor + 1).min(chars.len()))
    {
        if character == open {
            stack.push(index);
        } else if character == close {
            stack.pop();
        }
    }
    let left = stack.pop()?;
    let mut depth = 0;
    let right =
        chars
            .iter()
            .copied()
            .enumerate()
            .skip(left + 1)
            .find_map(|(index, character)| {
                if character == open {
                    depth += 1;
                } else if character == close {
                    if depth == 0 {
                        return Some(index);
                    }
                    depth -= 1;
                }
                None
            })?;
    Some(if around {
        left..right + 1
    } else {
        left + 1..right
    })
}

fn paragraph_object(text: &str, cursor: usize, around: bool) -> Range<usize> {
    let start = paragraph_back(text, cursor);
    let mut end = paragraph_forward(text, cursor);
    if around {
        end = line_end_with_newline(text, end);
    }
    start..end
}

fn indent_text(text: &str, indent: bool) -> String {
    text.split_inclusive('\n')
        .map(|line| {
            if indent {
                format!("    {line}")
            } else {
                line.strip_prefix("    ")
                    .or_else(|| line.strip_prefix('\t'))
                    .unwrap_or(line)
                    .to_owned()
            }
        })
        .collect()
}

fn clamp_normal(editor: &mut EditorSurface, text: &str) {
    let len = text.chars().count();
    if len == 0 {
        editor.set_selection(0, 0);
        return;
    }
    let start = line_start(text, editor.cursor().min(len));
    let end = line_end(text, editor.cursor().min(len));
    let cursor = if end > start {
        editor.cursor().min(end - 1)
    } else {
        start.min(len.saturating_sub(1))
    };
    editor.set_selection(cursor, cursor);
}

fn indexed_clamp_normal(editor: &mut EditorSurface, index: VimTextIndex<'_>) {
    if index.character_len == 0 {
        editor.set_selection(0, 0);
        return;
    }
    let start = index.line_start(editor.cursor());
    let end = index.line_end(editor.cursor());
    let cursor = if end > start {
        editor.cursor().min(end - 1)
    } else {
        start.min(index.character_len - 1)
    };
    editor.set_selection(cursor, cursor);
}

fn char_slice(text: &str, range: Range<usize>) -> &str {
    let start = text
        .char_indices()
        .nth(range.start)
        .map_or(text.len(), |(index, _)| index);
    let end = text
        .char_indices()
        .nth(range.end)
        .map_or(text.len(), |(index, _)| index);
    &text[start..end]
}

fn word_under_cursor(text: &str, cursor: usize) -> Option<String> {
    word_object(text, cursor, false, false).map(|range| char_slice(text, range).to_owned())
}

const fn reverse_search(direction: SearchDirection) -> SearchDirection {
    match direction {
        SearchDirection::Forward => SearchDirection::Backward,
        SearchDirection::Backward => SearchDirection::Forward,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExCommand {
    Write,
    Quit { force: bool },
    WriteQuit,
    Exit,
    Edit(String),
    NoHighlight,
    Line(usize),
}

pub fn parse_ex(input: &str) -> Result<ExCommand, String> {
    let input = input
        .trim()
        .strip_prefix(':')
        .unwrap_or(input.trim())
        .trim();
    match input {
        "w" => Ok(ExCommand::Write),
        "q" => Ok(ExCommand::Quit { force: false }),
        "q!" => Ok(ExCommand::Quit { force: true }),
        "wq" => Ok(ExCommand::WriteQuit),
        "x" => Ok(ExCommand::Exit),
        "noh" => Ok(ExCommand::NoHighlight),
        _ if input.starts_with("e ") && !input[2..].trim().is_empty() => {
            Ok(ExCommand::Edit(input[2..].trim().to_owned()))
        }
        _ if input.chars().all(|character| character.is_ascii_digit()) && !input.is_empty() => {
            let line = input.parse::<usize>().map_err(|error| error.to_string())?;
            if line == 0 {
                Err("line numbers are one-based".into())
            } else {
                Ok(ExCommand::Line(line))
            }
        }
        _ => Err(format!("unknown Ex command: {input}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delete_operator_composes_counts_with_word_motion() {
        let mut state = VimState::default();
        let mut session = VimSession::default();
        let mut editor = EditorSurface::default();
        let mut text = "one two three four five six seven".to_owned();

        state.execute(Command::VimCount2, &mut editor, &mut text, &mut session);
        state.execute(
            Command::VimOperatorDelete,
            &mut editor,
            &mut text,
            &mut session,
        );
        state.execute(Command::VimCount3, &mut editor, &mut text, &mut session);
        state.execute(
            Command::VimWordForward,
            &mut editor,
            &mut text,
            &mut session,
        );

        assert_eq!(text, "seven");
    }

    #[test]
    fn forward_and_file_end_operators_include_the_document_tail() {
        for command in [Command::VimWordForward, Command::VimFileEnd] {
            let mut state = VimState::default();
            let mut session = VimSession::default();
            let mut editor = EditorSurface::default();
            let mut text = "tail".to_owned();

            state.execute(
                Command::VimOperatorDelete,
                &mut editor,
                &mut text,
                &mut session,
            );
            state.execute(command, &mut editor, &mut text, &mut session);

            assert!(text.is_empty(), "{command:?} left {text:?}");
        }
    }

    #[test]
    fn doubled_yank_and_put_preserve_linewise_registers() {
        let mut state = VimState::default();
        let mut session = VimSession::default();
        let mut editor = EditorSurface::default();
        let mut text = "one\ntwo\n".to_owned();

        state.execute(
            Command::VimOperatorYank,
            &mut editor,
            &mut text,
            &mut session,
        );
        state.execute(
            Command::VimOperatorYank,
            &mut editor,
            &mut text,
            &mut session,
        );
        state.execute(Command::VimPutAfter, &mut editor, &mut text, &mut session);

        assert_eq!(text, "one\none\ntwo\n");
    }

    #[test]
    fn change_inner_word_enters_one_insert_transaction() {
        let mut state = VimState::default();
        let mut session = VimSession::default();
        let mut editor = EditorSurface::default();
        let mut text = "one two".to_owned();
        editor.set_selection(4, 4);

        state.execute(
            Command::VimOperatorChange,
            &mut editor,
            &mut text,
            &mut session,
        );
        state.execute(Command::VimTextInner, &mut editor, &mut text, &mut session);
        state.provide_character('w', &mut editor, &mut text, &mut session);
        assert!(state.insert_text("X", &mut editor, &mut text));
        state.execute(Command::VimNormal, &mut editor, &mut text, &mut session);

        assert_eq!(text, "one X");
        assert!(editor.undo(&mut text));
        assert_eq!(text, "one two");
    }

    #[test]
    fn linewise_change_preserves_the_line_boundary_and_undo_group() {
        let mut state = VimState::default();
        let mut session = VimSession::default();
        let mut editor = EditorSurface::default();
        let mut text = "one\ntwo".to_owned();

        state.execute(
            Command::VimOperatorChange,
            &mut editor,
            &mut text,
            &mut session,
        );
        state.execute(
            Command::VimOperatorChange,
            &mut editor,
            &mut text,
            &mut session,
        );
        assert!(state.insert_text("X", &mut editor, &mut text));
        state.execute(Command::VimNormal, &mut editor, &mut text, &mut session);

        assert_eq!(text, "X\ntwo");
        assert!(editor.undo(&mut text));
        assert_eq!(text, "one\ntwo");
    }

    #[test]
    fn insert_session_undo_and_dot_repeat_are_grouped() {
        let mut state = VimState::default();
        let mut session = VimSession::default();
        let mut editor = EditorSurface::default();
        let mut text = String::new();
        state.execute(Command::VimInsert, &mut editor, &mut text, &mut session);
        assert!(state.insert_text("abc", &mut editor, &mut text));
        state.execute(Command::VimNormal, &mut editor, &mut text, &mut session);
        state.execute(Command::VimUndo, &mut editor, &mut text, &mut session);

        state.execute(Command::VimRepeat, &mut editor, &mut text, &mut session);

        assert_eq!(text, "abc");
    }

    #[test]
    fn vertical_motion_restores_the_preferred_column_after_a_short_line() {
        let mut state = VimState::default();
        let mut session = VimSession::default();
        let mut editor = EditorSurface::default();
        let mut text = "abcd\nx\nwxyz".to_owned();
        editor.set_selection(3, 3);

        state.execute(Command::VimMoveDown, &mut editor, &mut text, &mut session);
        assert_eq!(editor.cursor(), 5);
        state.execute(Command::VimMoveDown, &mut editor, &mut text, &mut session);

        assert_eq!(editor.cursor(), 10);
    }

    #[test]
    fn indexed_motions_match_the_fallback_on_unicode_text() {
        let original = "alpha βeta\n  gamma delta\n\nz";
        let mut line_starts = vec![0];
        let mut line_byte_starts = vec![0];
        let mut character_len = 0;
        for (byte, character) in original.char_indices() {
            character_len += 1;
            if character == '\n' {
                line_starts.push(character_len);
                line_byte_starts.push(byte + 1);
            }
        }

        for (command, cursor) in [
            (Command::VimWordForward, 0),
            (Command::VimWordBack, character_len),
            (Command::VimMoveDown, 7),
            (Command::VimFirstNonBlank, 12),
            (Command::VimLineEnd, 12),
            (Command::VimParagraphForward, 0),
            (Command::VimParagraphBack, character_len),
        ] {
            let mut fallback = VimState::default();
            let mut indexed = VimState::default();
            let mut fallback_editor = EditorSurface::default();
            let mut indexed_editor = EditorSurface::default();
            let mut fallback_session = VimSession::default();
            let mut indexed_session = VimSession::default();
            let mut fallback_text = original.to_owned();
            let mut indexed_text = original.to_owned();
            fallback_editor.set_selection(cursor, cursor);
            indexed_editor.set_selection(cursor, cursor);

            fallback.execute(
                command,
                &mut fallback_editor,
                &mut fallback_text,
                &mut fallback_session,
            );
            indexed.execute_indexed(
                command,
                &mut indexed_editor,
                &mut indexed_text,
                &mut indexed_session,
                VimTextIndex::new(&line_starts, &line_byte_starts, character_len),
            );

            assert_eq!(
                indexed_editor.cursor(),
                fallback_editor.cursor(),
                "{command:?}"
            );
        }

        let mut fallback = VimState::default();
        let mut indexed = VimState::default();
        let mut fallback_editor = EditorSurface::default();
        let mut indexed_editor = EditorSurface::default();
        let mut fallback_session = VimSession::default();
        let mut indexed_session = VimSession::default();
        let mut fallback_text = original.to_owned();
        let mut indexed_text = original.to_owned();
        fallback.execute(
            Command::VimFindForward,
            &mut fallback_editor,
            &mut fallback_text,
            &mut fallback_session,
        );
        indexed.execute_indexed(
            Command::VimFindForward,
            &mut indexed_editor,
            &mut indexed_text,
            &mut indexed_session,
            VimTextIndex::new(&line_starts, &line_byte_starts, character_len),
        );
        fallback.provide_character(
            'β',
            &mut fallback_editor,
            &mut fallback_text,
            &mut fallback_session,
        );
        indexed.provide_character_indexed(
            'β',
            &mut indexed_editor,
            &mut indexed_text,
            &mut indexed_session,
            VimTextIndex::new(&line_starts, &line_byte_starts, character_len),
        );

        assert_eq!(indexed_editor.cursor(), fallback_editor.cursor());
    }

    #[test]
    fn indexed_text_objects_match_the_fallback() {
        for (text, cursor, object) in [
            ("alpha βeta", 2, 'w'),
            ("say \"hello\" now", 7, '"'),
            ("a (b [c] d) e", 8, '('),
            ("one\n\nthree\n", 0, 'p'),
        ] {
            let mut line_starts = vec![0];
            let mut line_byte_starts = vec![0];
            let mut character_len = 0;
            for (byte, character) in text.char_indices() {
                character_len += 1;
                if character == '\n' {
                    line_starts.push(character_len);
                    line_byte_starts.push(byte + 1);
                }
            }
            let index = VimTextIndex::new(&line_starts, &line_byte_starts, character_len);
            for around in [false, true] {
                assert_eq!(
                    indexed_text_object(text, cursor, object, around, index),
                    text_object(text, cursor, object, around),
                    "{object} around={around}"
                );
            }
        }
    }

    #[test]
    fn joined_lines_and_repeated_changes_are_single_undo_steps() {
        let mut state = VimState::default();
        let mut session = VimSession::default();
        let mut editor = EditorSurface::default();
        let mut text = "one\n  two\nthree".to_owned();

        state.execute(Command::VimJoinLines, &mut editor, &mut text, &mut session);
        assert_eq!(text, "one two\nthree");
        assert!(editor.undo(&mut text));
        assert_eq!(text, "one\n  two\nthree");

        editor.set_selection(0, 0);
        state.execute(
            Command::VimOperatorChange,
            &mut editor,
            &mut text,
            &mut session,
        );
        state.execute(Command::VimTextInner, &mut editor, &mut text, &mut session);
        state.provide_character('w', &mut editor, &mut text, &mut session);
        assert!(state.insert_text("X", &mut editor, &mut text));
        state.execute(Command::VimNormal, &mut editor, &mut text, &mut session);
        editor.set_selection(2, 2);
        state.execute(Command::VimRepeat, &mut editor, &mut text, &mut session);

        assert_eq!(text, "X\n  X\nthree");
    }

    #[test]
    fn pointer_selections_use_the_matching_visual_mode_and_exact_range() {
        let mut state = VimState::default();
        let mut session = VimSession::default();
        let mut editor = EditorSurface::default();
        let mut text = "alpha beta\ngamma".to_owned();

        editor.set_selection(6, 10);
        state.sync_pointer_selection(&editor, false);
        assert_eq!(state.mode(), VimMode::VisualCharacter);
        state.execute(
            Command::VimOperatorDelete,
            &mut editor,
            &mut text,
            &mut session,
        );
        assert_eq!(text, "alpha \ngamma");

        editor.set_selection(0, 7);
        state.sync_pointer_selection(&editor, true);
        assert_eq!(state.mode(), VimMode::VisualLine);
        state.execute(
            Command::VimOperatorDelete,
            &mut editor,
            &mut text,
            &mut session,
        );
        assert_eq!(text, "gamma");
    }

    #[test]
    fn ex_parser_accepts_only_the_documented_safe_subset() {
        assert_eq!(parse_ex(":wq"), Ok(ExCommand::WriteQuit));
        assert_eq!(
            parse_ex(" e src/main.rs "),
            Ok(ExCommand::Edit("src/main.rs".into()))
        );
        assert_eq!(parse_ex("42"), Ok(ExCommand::Line(42)));
        assert!(parse_ex("0").is_err());
        assert!(parse_ex(":e").is_err());
        assert!(parse_ex(":set number").is_err());
    }
}
