use std::{
    collections::{BTreeMap, HashSet},
    fmt,
    time::Duration,
};

use egui::{Key, Modifiers};
use serde::{Deserialize, Serialize};

pub const BUILTIN_VSCODE: &str = "vscode";
pub const BUILTIN_VIM: &str = "vim";
pub const MAX_CUSTOM_PROFILES: usize = 16;
pub const MAX_CUSTOM_DEVIATIONS: usize = 256;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Macos,
    Windows,
    Linux,
}

impl Platform {
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Macos
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else {
            Self::Linux
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Behavior {
    #[default]
    Standard,
    Vim,
}

impl Behavior {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Standard => "Standard",
            Self::Vim => "Vim modal",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Scope {
    Global,
    DocumentEditor,
    VimNormal,
    VimInsert,
    VimReplace,
    VimVisual,
    VimOperator,
    FilesTree,
    Find,
    ProjectSearch,
    Agent,
    Terminal,
    Settings,
}

impl Scope {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Global => "Global",
            Self::DocumentEditor => "Document editor",
            Self::VimNormal => "Vim Normal",
            Self::VimInsert => "Vim Insert",
            Self::VimReplace => "Vim Replace",
            Self::VimVisual => "Vim Visual",
            Self::VimOperator => "Vim Operator-pending",
            Self::FilesTree => "Files tree",
            Self::Find => "Find",
            Self::ProjectSearch => "Project search",
            Self::Agent => "Agent",
            Self::Terminal => "Terminal",
            Self::Settings => "Settings",
        }
    }

    pub const fn is_vim(self) -> bool {
        matches!(
            self,
            Self::VimNormal
                | Self::VimInsert
                | Self::VimReplace
                | Self::VimVisual
                | Self::VimOperator
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Command {
    AppOpenSettings,
    AppOpenKeybindings,
    AppIncreaseUiScale,
    AppDecreaseUiScale,
    AppCloseWindow,
    AppToggleAgentSidebar,
    AppToggleAgenticView,
    FileSave,
    FileSaveAndClose,
    FileCloseActive,
    FileFocusPane1,
    FileFocusPane2,
    FileFocusPane3,
    FileFocusPane4,
    FileFocusPane5,
    FileFocusPane6,
    FileFocusPane7,
    FileFocusPane8,
    FileFocusPane9,
    FileSplitEditor,
    FileFocusNextPane,
    FileFocusPreviousPane,
    FileFocusLeftPane,
    FileFocusRightPane,
    ViewToggleSidebar,
    ViewFocusExplorer,
    ViewToggleTerminal,
    ViewToggleMarkdownPreview,
    SearchFind,
    SearchNext,
    SearchPrevious,
    SearchProject,
    SearchClose,
    EditorCopy,
    EditorCut,
    EditorPaste,
    EditorUndo,
    EditorRedo,
    EditorSelectAll,
    EditorDeleteLeft,
    EditorDeleteRight,
    EditorInsertLineBreak,
    EditorIndent,
    EditorOutdent,
    EditorCursorLeft,
    EditorCursorRight,
    EditorCursorUp,
    EditorCursorDown,
    EditorCursorWordLeft,
    EditorCursorWordRight,
    EditorCursorLineStart,
    EditorCursorLineEnd,
    EditorCursorPageUp,
    EditorCursorPageDown,
    EditorCursorDocumentStart,
    EditorCursorDocumentEnd,
    EditorSelectLeft,
    EditorSelectRight,
    EditorSelectUp,
    EditorSelectDown,
    EditorSelectWordLeft,
    EditorSelectWordRight,
    EditorSelectLineStart,
    EditorSelectLineEnd,
    EditorSelectPageUp,
    EditorSelectPageDown,
    EditorSelectDocumentStart,
    EditorSelectDocumentEnd,
    TreeMoveUp,
    TreeMoveDown,
    TreeExpand,
    TreeCollapse,
    TreeOpen,
    EditorTriggerSuggest,
    EditorGoToDefinition,
    EditorNextDiagnostic,
    EditorPreviousDiagnostic,
    VimNormal,
    VimInsert,
    VimInsertLineStart,
    VimAppend,
    VimAppendLineEnd,
    VimOpenBelow,
    VimOpenAbove,
    VimSubstituteChar,
    VimSubstituteLine,
    VimChangeLineEnd,
    VimReplaceMode,
    VimVisualCharacter,
    VimVisualLine,
    VimMoveLeft,
    VimMoveRight,
    VimMoveUp,
    VimMoveDown,
    VimLineStart,
    VimFirstNonBlank,
    VimLineEnd,
    VimFileStart,
    VimFileEnd,
    VimWordForward,
    VimBigWordForward,
    VimWordEnd,
    VimBigWordEnd,
    VimWordBack,
    VimBigWordBack,
    VimParagraphBack,
    VimParagraphForward,
    VimFindForward,
    VimFindBackward,
    VimTillForward,
    VimTillBackward,
    VimRepeatFind,
    VimReverseFind,
    VimOperatorDelete,
    VimOperatorChange,
    VimOperatorYank,
    VimOperatorIndent,
    VimOperatorOutdent,
    VimTextInner,
    VimTextAround,
    VimDeleteCharacter,
    VimDeleteCharacterLeft,
    VimDeleteLineEnd,
    VimReplaceCharacter,
    VimJoinLines,
    VimToggleCase,
    VimPutAfter,
    VimPutBefore,
    VimUndo,
    VimRedo,
    VimRepeat,
    VimSearchForward,
    VimSearchBackward,
    VimSearchNext,
    VimSearchPrevious,
    VimSearchWordForward,
    VimSearchWordBackward,
    VimRegisterSystem,
    VimEx,
    VimCount0,
    VimCount1,
    VimCount2,
    VimCount3,
    VimCount4,
    VimCount5,
    VimCount6,
    VimCount7,
    VimCount8,
    VimCount9,
}

#[derive(Clone, Copy, Debug)]
pub struct CommandInfo {
    pub command: Command,
    pub id: &'static str,
    pub label: &'static str,
    pub category: &'static str,
    pub scopes: &'static [Scope],
    pub repeatable: bool,
    pub changes_text: bool,
}

const GLOBAL: &[Scope] = &[Scope::Global];
const GLOBAL_VIM_NORMAL: &[Scope] = &[Scope::Global, Scope::VimNormal];
const DOCUMENT: &[Scope] = &[
    Scope::DocumentEditor,
    Scope::VimNormal,
    Scope::VimInsert,
    Scope::VimReplace,
    Scope::VimVisual,
    Scope::VimOperator,
];
const DOCUMENT_AND_FIND: &[Scope] = &[Scope::DocumentEditor, Scope::Find, Scope::Agent];
const TREE: &[Scope] = &[Scope::FilesTree];
const VIM_COMMAND: &[Scope] = &[Scope::VimNormal, Scope::VimVisual, Scope::VimOperator];
const VIM_NORMAL: &[Scope] = &[Scope::VimNormal];
const VIM_NORMAL_VISUAL: &[Scope] = &[Scope::VimNormal, Scope::VimVisual];
const VIM_ALL: &[Scope] = &[
    Scope::VimNormal,
    Scope::VimInsert,
    Scope::VimReplace,
    Scope::VimVisual,
    Scope::VimOperator,
];

macro_rules! info {
    ($variant:ident, $id:literal, $label:literal, $category:literal, $scopes:expr, $repeat:expr, $text:expr) => {
        CommandInfo {
            command: Command::$variant,
            id: $id,
            label: $label,
            category: $category,
            scopes: $scopes,
            repeatable: $repeat,
            changes_text: $text,
        }
    };
}

pub static CATALOG: &[CommandInfo] = &[
    info!(
        AppOpenSettings,
        "application.openSettings", "Open Settings", "Application", GLOBAL, false, false
    ),
    info!(
        AppOpenKeybindings,
        "application.openKeyboardShortcuts",
        "Open Keyboard Shortcuts",
        "Application",
        GLOBAL,
        false,
        false
    ),
    info!(
        AppIncreaseUiScale,
        "application.increaseUiScale", "Increase UI Scale", "Application", GLOBAL, false, false
    ),
    info!(
        AppDecreaseUiScale,
        "application.decreaseUiScale", "Decrease UI Scale", "Application", GLOBAL, false, false
    ),
    info!(
        AppCloseWindow,
        "application.closeWindow", "Close Window", "Application", GLOBAL, false, false
    ),
    info!(
        AppToggleAgentSidebar,
        "application.toggleAgentSidebar",
        "Toggle Agent Sidebar",
        "Application",
        GLOBAL,
        false,
        false
    ),
    info!(
        AppToggleAgenticView,
        "application.toggleAgenticView", "Toggle Agentic View", "Application", GLOBAL, false, false
    ),
    info!(FileSave, "file.save", "Save", "Files", GLOBAL, false, false),
    info!(
        FileSaveAndClose,
        "file.saveAndClose", "Save and Close", "Files", GLOBAL, false, false
    ),
    info!(
        FileCloseActive,
        "file.closeActiveEditor", "Close Active Editor", "Files", GLOBAL_VIM_NORMAL, false, false
    ),
    info!(
        FileFocusPane1,
        "file.focusPane1", "Focus Editor Pane 1", "Files", GLOBAL, false, false
    ),
    info!(
        FileFocusPane2,
        "file.focusPane2", "Focus Editor Pane 2", "Files", GLOBAL, false, false
    ),
    info!(
        FileFocusPane3,
        "file.focusPane3", "Focus Editor Pane 3", "Files", GLOBAL, false, false
    ),
    info!(
        FileFocusPane4,
        "file.focusPane4", "Focus Editor Pane 4", "Files", GLOBAL, false, false
    ),
    info!(
        FileFocusPane5,
        "file.focusPane5", "Focus Editor Pane 5", "Files", GLOBAL, false, false
    ),
    info!(
        FileFocusPane6,
        "file.focusPane6", "Focus Editor Pane 6", "Files", GLOBAL, false, false
    ),
    info!(
        FileFocusPane7,
        "file.focusPane7", "Focus Editor Pane 7", "Files", GLOBAL, false, false
    ),
    info!(
        FileFocusPane8,
        "file.focusPane8", "Focus Editor Pane 8", "Files", GLOBAL, false, false
    ),
    info!(
        FileFocusPane9,
        "file.focusPane9", "Focus Editor Pane 9", "Files", GLOBAL, false, false
    ),
    info!(
        FileSplitEditor,
        "file.splitEditor", "Split Editor", "Files", GLOBAL, false, false
    ),
    info!(
        FileFocusNextPane,
        "file.focusNextPane", "Focus Next Pane", "Files", GLOBAL_VIM_NORMAL, true, false
    ),
    info!(
        FileFocusPreviousPane,
        "file.focusPreviousPane", "Focus Previous Pane", "Files", GLOBAL_VIM_NORMAL, true, false
    ),
    info!(
        FileFocusLeftPane,
        "file.focusLeftPane", "Focus Left Pane", "Files", GLOBAL_VIM_NORMAL, true, false
    ),
    info!(
        FileFocusRightPane,
        "file.focusRightPane", "Focus Right Pane", "Files", GLOBAL_VIM_NORMAL, true, false
    ),
    info!(
        ViewToggleSidebar,
        "workbench.toggleSidebar", "Toggle Files Sidebar", "View", GLOBAL, false, false
    ),
    info!(
        ViewFocusExplorer,
        "workbench.focusExplorer", "Focus Files Explorer", "View", GLOBAL, false, false
    ),
    info!(
        ViewToggleTerminal,
        "workbench.toggleTerminal", "Toggle Terminal", "View", GLOBAL, false, false
    ),
    info!(
        ViewToggleMarkdownPreview,
        "workbench.toggleMarkdownPreview", "Toggle Markdown Preview", "View", GLOBAL, false, false
    ),
    info!(
        SearchFind,
        "search.findInFile", "Find in File", "Search", GLOBAL, false, false
    ),
    info!(
        SearchNext,
        "search.findNext", "Find Next", "Search", DOCUMENT_AND_FIND, true, false
    ),
    info!(
        SearchPrevious,
        "search.findPrevious", "Find Previous", "Search", DOCUMENT_AND_FIND, true, false
    ),
    info!(
        SearchProject,
        "search.searchProject", "Search Project", "Search", GLOBAL, false, false
    ),
    info!(
        SearchClose,
        "search.close",
        "Close Search",
        "Search",
        &[Scope::Find, Scope::ProjectSearch, Scope::Agent],
        false,
        false
    ),
    info!(
        EditorCopy,
        "editor.copy", "Copy", "Editor", DOCUMENT, false, false
    ),
    info!(
        EditorCut,
        "editor.cut", "Cut", "Editor", DOCUMENT, false, true
    ),
    info!(
        EditorPaste,
        "editor.paste", "Paste", "Editor", DOCUMENT, false, true
    ),
    info!(
        EditorUndo,
        "editor.undo", "Undo", "Editor", DOCUMENT, false, true
    ),
    info!(
        EditorRedo,
        "editor.redo", "Redo", "Editor", DOCUMENT, false, true
    ),
    info!(
        EditorSelectAll,
        "editor.selectAll", "Select All", "Editor", DOCUMENT, false, false
    ),
    info!(
        EditorDeleteLeft,
        "editor.deleteLeft", "Delete Left", "Editor", DOCUMENT, true, true
    ),
    info!(
        EditorDeleteRight,
        "editor.deleteRight", "Delete Right", "Editor", DOCUMENT, true, true
    ),
    info!(
        EditorInsertLineBreak,
        "editor.insertLineBreak", "Insert Line Break", "Editor", DOCUMENT, true, true
    ),
    info!(
        EditorIndent,
        "editor.indent", "Indent", "Editor", DOCUMENT, true, true
    ),
    info!(
        EditorOutdent,
        "editor.outdent", "Outdent", "Editor", DOCUMENT, true, true
    ),
    info!(
        EditorCursorLeft,
        "editor.cursorLeft", "Cursor Left", "Editor", DOCUMENT, true, false
    ),
    info!(
        EditorCursorRight,
        "editor.cursorRight", "Cursor Right", "Editor", DOCUMENT, true, false
    ),
    info!(
        EditorCursorUp,
        "editor.cursorUp", "Cursor Up", "Editor", DOCUMENT, true, false
    ),
    info!(
        EditorCursorDown,
        "editor.cursorDown", "Cursor Down", "Editor", DOCUMENT, true, false
    ),
    info!(
        EditorCursorWordLeft,
        "editor.cursorWordLeft", "Cursor Word Left", "Editor", DOCUMENT, true, false
    ),
    info!(
        EditorCursorWordRight,
        "editor.cursorWordRight", "Cursor Word Right", "Editor", DOCUMENT, true, false
    ),
    info!(
        EditorCursorLineStart,
        "editor.cursorLineStart", "Cursor Line Start", "Editor", DOCUMENT, true, false
    ),
    info!(
        EditorCursorLineEnd,
        "editor.cursorLineEnd", "Cursor Line End", "Editor", DOCUMENT, true, false
    ),
    info!(
        EditorCursorPageUp,
        "editor.cursorPageUp", "Cursor Page Up", "Editor", DOCUMENT, true, false
    ),
    info!(
        EditorCursorPageDown,
        "editor.cursorPageDown", "Cursor Page Down", "Editor", DOCUMENT, true, false
    ),
    info!(
        EditorCursorDocumentStart,
        "editor.cursorDocumentStart", "Cursor Document Start", "Editor", DOCUMENT, true, false
    ),
    info!(
        EditorCursorDocumentEnd,
        "editor.cursorDocumentEnd", "Cursor Document End", "Editor", DOCUMENT, true, false
    ),
    info!(
        EditorSelectLeft,
        "editor.selectLeft", "Select Left", "Editor", DOCUMENT, true, false
    ),
    info!(
        EditorSelectRight,
        "editor.selectRight", "Select Right", "Editor", DOCUMENT, true, false
    ),
    info!(
        EditorSelectUp,
        "editor.selectUp", "Select Up", "Editor", DOCUMENT, true, false
    ),
    info!(
        EditorSelectDown,
        "editor.selectDown", "Select Down", "Editor", DOCUMENT, true, false
    ),
    info!(
        EditorSelectWordLeft,
        "editor.selectWordLeft", "Select Word Left", "Editor", DOCUMENT, true, false
    ),
    info!(
        EditorSelectWordRight,
        "editor.selectWordRight", "Select Word Right", "Editor", DOCUMENT, true, false
    ),
    info!(
        EditorSelectLineStart,
        "editor.selectLineStart", "Select to Line Start", "Editor", DOCUMENT, true, false
    ),
    info!(
        EditorSelectLineEnd,
        "editor.selectLineEnd", "Select to Line End", "Editor", DOCUMENT, true, false
    ),
    info!(
        EditorSelectPageUp,
        "editor.selectPageUp", "Select Page Up", "Editor", DOCUMENT, true, false
    ),
    info!(
        EditorSelectPageDown,
        "editor.selectPageDown", "Select Page Down", "Editor", DOCUMENT, true, false
    ),
    info!(
        EditorSelectDocumentStart,
        "editor.selectDocumentStart", "Select to Document Start", "Editor", DOCUMENT, true, false
    ),
    info!(
        EditorSelectDocumentEnd,
        "editor.selectDocumentEnd", "Select to Document End", "Editor", DOCUMENT, true, false
    ),
    info!(
        TreeMoveUp,
        "tree.moveUp", "Tree: Move Up", "Tree", TREE, true, false
    ),
    info!(
        TreeMoveDown,
        "tree.moveDown", "Tree: Move Down", "Tree", TREE, true, false
    ),
    info!(
        TreeExpand,
        "tree.expand", "Tree: Expand", "Tree", TREE, true, false
    ),
    info!(
        TreeCollapse,
        "tree.collapse", "Tree: Collapse", "Tree", TREE, true, false
    ),
    info!(
        TreeOpen,
        "tree.open", "Tree: Open Selected", "Tree", TREE, false, false
    ),
    info!(
        EditorTriggerSuggest,
        "editor.triggerSuggest", "Trigger Suggestions", "Editor", DOCUMENT, false, false
    ),
    info!(
        EditorGoToDefinition,
        "editor.goToDefinition", "Go to Definition", "Editor", DOCUMENT, false, false
    ),
    info!(
        EditorNextDiagnostic,
        "editor.nextDiagnostic", "Next Diagnostic", "Editor", DOCUMENT, true, false
    ),
    info!(
        EditorPreviousDiagnostic,
        "editor.previousDiagnostic", "Previous Diagnostic", "Editor", DOCUMENT, true, false
    ),
    info!(
        VimNormal,
        "vim.mode.normal", "Vim: Normal Mode", "Vim", VIM_ALL, false, false
    ),
    info!(
        VimInsert,
        "vim.mode.insert", "Vim: Insert", "Vim", VIM_NORMAL, false, false
    ),
    info!(
        VimInsertLineStart,
        "vim.mode.insertLineStart", "Vim: Insert at Line Start", "Vim", VIM_NORMAL, false, false
    ),
    info!(
        VimAppend,
        "vim.mode.append", "Vim: Append", "Vim", VIM_NORMAL, false, false
    ),
    info!(
        VimAppendLineEnd,
        "vim.mode.appendLineEnd", "Vim: Append at Line End", "Vim", VIM_NORMAL, false, false
    ),
    info!(
        VimOpenBelow,
        "vim.mode.openBelow", "Vim: Open Line Below", "Vim", VIM_NORMAL, false, true
    ),
    info!(
        VimOpenAbove,
        "vim.mode.openAbove", "Vim: Open Line Above", "Vim", VIM_NORMAL, false, true
    ),
    info!(
        VimSubstituteChar,
        "vim.change.substituteCharacter",
        "Vim: Substitute Character",
        "Vim",
        VIM_NORMAL,
        false,
        true
    ),
    info!(
        VimSubstituteLine,
        "vim.change.substituteLine", "Vim: Substitute Line", "Vim", VIM_NORMAL, false, true
    ),
    info!(
        VimChangeLineEnd,
        "vim.change.toLineEnd", "Vim: Change to Line End", "Vim", VIM_NORMAL, false, true
    ),
    info!(
        VimReplaceMode,
        "vim.mode.replace", "Vim: Replace Mode", "Vim", VIM_NORMAL, false, false
    ),
    info!(
        VimVisualCharacter,
        "vim.mode.visualCharacter", "Vim: Visual Character", "Vim", VIM_NORMAL_VISUAL, false, false
    ),
    info!(
        VimVisualLine,
        "vim.mode.visualLine", "Vim: Visual Line", "Vim", VIM_NORMAL_VISUAL, false, false
    ),
    info!(
        VimMoveLeft,
        "vim.motion.left", "Vim: Left", "Vim", VIM_COMMAND, true, false
    ),
    info!(
        VimMoveRight,
        "vim.motion.right", "Vim: Right", "Vim", VIM_COMMAND, true, false
    ),
    info!(
        VimMoveUp,
        "vim.motion.up", "Vim: Up", "Vim", VIM_COMMAND, true, false
    ),
    info!(
        VimMoveDown,
        "vim.motion.down", "Vim: Down", "Vim", VIM_COMMAND, true, false
    ),
    info!(
        VimLineStart,
        "vim.motion.lineStart", "Vim: Line Start", "Vim", VIM_COMMAND, true, false
    ),
    info!(
        VimFirstNonBlank,
        "vim.motion.firstNonBlank", "Vim: First Non-blank", "Vim", VIM_COMMAND, true, false
    ),
    info!(
        VimLineEnd,
        "vim.motion.lineEnd", "Vim: Line End", "Vim", VIM_COMMAND, true, false
    ),
    info!(
        VimFileStart,
        "vim.motion.fileStart", "Vim: File Start", "Vim", VIM_COMMAND, true, false
    ),
    info!(
        VimFileEnd,
        "vim.motion.fileEnd", "Vim: File End", "Vim", VIM_COMMAND, true, false
    ),
    info!(
        VimWordForward,
        "vim.motion.wordForward", "Vim: Word Forward", "Vim", VIM_COMMAND, true, false
    ),
    info!(
        VimBigWordForward,
        "vim.motion.bigWordForward", "Vim: WORD Forward", "Vim", VIM_COMMAND, true, false
    ),
    info!(
        VimWordEnd,
        "vim.motion.wordEnd", "Vim: Word End", "Vim", VIM_COMMAND, true, false
    ),
    info!(
        VimBigWordEnd,
        "vim.motion.bigWordEnd", "Vim: WORD End", "Vim", VIM_COMMAND, true, false
    ),
    info!(
        VimWordBack,
        "vim.motion.wordBack", "Vim: Word Back", "Vim", VIM_COMMAND, true, false
    ),
    info!(
        VimBigWordBack,
        "vim.motion.bigWordBack", "Vim: WORD Back", "Vim", VIM_COMMAND, true, false
    ),
    info!(
        VimParagraphBack,
        "vim.motion.paragraphBack", "Vim: Paragraph Back", "Vim", VIM_COMMAND, true, false
    ),
    info!(
        VimParagraphForward,
        "vim.motion.paragraphForward", "Vim: Paragraph Forward", "Vim", VIM_COMMAND, true, false
    ),
    info!(
        VimFindForward,
        "vim.motion.findForward", "Vim: Find Character Forward", "Vim", VIM_COMMAND, true, false
    ),
    info!(
        VimFindBackward,
        "vim.motion.findBackward", "Vim: Find Character Backward", "Vim", VIM_COMMAND, true, false
    ),
    info!(
        VimTillForward,
        "vim.motion.tillForward", "Vim: Till Character Forward", "Vim", VIM_COMMAND, true, false
    ),
    info!(
        VimTillBackward,
        "vim.motion.tillBackward", "Vim: Till Character Backward", "Vim", VIM_COMMAND, true, false
    ),
    info!(
        VimRepeatFind,
        "vim.motion.repeatFind", "Vim: Repeat Character Find", "Vim", VIM_COMMAND, true, false
    ),
    info!(
        VimReverseFind,
        "vim.motion.reverseFind", "Vim: Reverse Character Find", "Vim", VIM_COMMAND, true, false
    ),
    info!(
        VimOperatorDelete,
        "vim.operator.delete", "Vim: Delete Operator", "Vim", VIM_COMMAND, false, true
    ),
    info!(
        VimOperatorChange,
        "vim.operator.change", "Vim: Change Operator", "Vim", VIM_COMMAND, false, true
    ),
    info!(
        VimOperatorYank,
        "vim.operator.yank", "Vim: Yank Operator", "Vim", VIM_COMMAND, false, false
    ),
    info!(
        VimOperatorIndent,
        "vim.operator.indent", "Vim: Indent Operator", "Vim", VIM_COMMAND, false, true
    ),
    info!(
        VimOperatorOutdent,
        "vim.operator.outdent", "Vim: Outdent Operator", "Vim", VIM_COMMAND, false, true
    ),
    info!(
        VimTextInner,
        "vim.textObject.inner",
        "Vim: Inner Text Object",
        "Vim",
        &[Scope::VimOperator],
        false,
        false
    ),
    info!(
        VimTextAround,
        "vim.textObject.around",
        "Vim: Around Text Object",
        "Vim",
        &[Scope::VimOperator],
        false,
        false
    ),
    info!(
        VimDeleteCharacter,
        "vim.change.deleteCharacter", "Vim: Delete Character", "Vim", VIM_NORMAL_VISUAL, true, true
    ),
    info!(
        VimDeleteCharacterLeft,
        "vim.change.deleteCharacterLeft",
        "Vim: Delete Character Left",
        "Vim",
        VIM_NORMAL,
        true,
        true
    ),
    info!(
        VimDeleteLineEnd,
        "vim.change.deleteToLineEnd", "Vim: Delete to Line End", "Vim", VIM_NORMAL, false, true
    ),
    info!(
        VimReplaceCharacter,
        "vim.change.replaceCharacter", "Vim: Replace Character", "Vim", VIM_NORMAL, false, true
    ),
    info!(
        VimJoinLines,
        "vim.change.joinLines", "Vim: Join Lines", "Vim", VIM_NORMAL, false, true
    ),
    info!(
        VimToggleCase,
        "vim.change.toggleCase", "Vim: Toggle Case", "Vim", VIM_NORMAL_VISUAL, true, true
    ),
    info!(
        VimPutAfter,
        "vim.change.putAfter", "Vim: Put After", "Vim", VIM_NORMAL_VISUAL, false, true
    ),
    info!(
        VimPutBefore,
        "vim.change.putBefore", "Vim: Put Before", "Vim", VIM_NORMAL_VISUAL, false, true
    ),
    info!(
        VimUndo,
        "vim.history.undo", "Vim: Undo", "Vim", VIM_NORMAL, false, true
    ),
    info!(
        VimRedo,
        "vim.history.redo", "Vim: Redo", "Vim", VIM_NORMAL, false, true
    ),
    info!(
        VimRepeat,
        "vim.history.repeat", "Vim: Repeat Last Change", "Vim", VIM_NORMAL, false, true
    ),
    info!(
        VimSearchForward,
        "vim.search.forward", "Vim: Search Forward", "Vim", VIM_NORMAL_VISUAL, false, false
    ),
    info!(
        VimSearchBackward,
        "vim.search.backward", "Vim: Search Backward", "Vim", VIM_NORMAL_VISUAL, false, false
    ),
    info!(
        VimSearchNext,
        "vim.search.next", "Vim: Next Search Result", "Vim", VIM_NORMAL_VISUAL, true, false
    ),
    info!(
        VimSearchPrevious,
        "vim.search.previous", "Vim: Previous Search Result", "Vim", VIM_NORMAL_VISUAL, true, false
    ),
    info!(
        VimSearchWordForward,
        "vim.search.wordForward", "Vim: Search Word Forward", "Vim", VIM_NORMAL, false, false
    ),
    info!(
        VimSearchWordBackward,
        "vim.search.wordBackward", "Vim: Search Word Backward", "Vim", VIM_NORMAL, false, false
    ),
    info!(
        VimRegisterSystem,
        "vim.register.system", "Vim: System Clipboard Register", "Vim", VIM_COMMAND, false, false
    ),
    info!(
        VimEx,
        "vim.ex.open", "Vim: Ex Command", "Vim", VIM_NORMAL, false, false
    ),
    info!(
        VimCount0,
        "vim.count.0", "Vim: Count 0", "Vim", VIM_COMMAND, true, false
    ),
    info!(
        VimCount1,
        "vim.count.1", "Vim: Count 1", "Vim", VIM_COMMAND, true, false
    ),
    info!(
        VimCount2,
        "vim.count.2", "Vim: Count 2", "Vim", VIM_COMMAND, true, false
    ),
    info!(
        VimCount3,
        "vim.count.3", "Vim: Count 3", "Vim", VIM_COMMAND, true, false
    ),
    info!(
        VimCount4,
        "vim.count.4", "Vim: Count 4", "Vim", VIM_COMMAND, true, false
    ),
    info!(
        VimCount5,
        "vim.count.5", "Vim: Count 5", "Vim", VIM_COMMAND, true, false
    ),
    info!(
        VimCount6,
        "vim.count.6", "Vim: Count 6", "Vim", VIM_COMMAND, true, false
    ),
    info!(
        VimCount7,
        "vim.count.7", "Vim: Count 7", "Vim", VIM_COMMAND, true, false
    ),
    info!(
        VimCount8,
        "vim.count.8", "Vim: Count 8", "Vim", VIM_COMMAND, true, false
    ),
    info!(
        VimCount9,
        "vim.count.9", "Vim: Count 9", "Vim", VIM_COMMAND, true, false
    ),
];

impl Command {
    pub fn from_id(id: &str) -> Option<Self> {
        CATALOG
            .iter()
            .find(|info| info.id == id)
            .map(|info| info.command)
    }

    pub fn info(self) -> &'static CommandInfo {
        CATALOG
            .iter()
            .find(|info| info.command == self)
            .expect("catalog covers every command")
    }

    pub fn id(self) -> &'static str {
        self.info().id
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Stroke {
    pub key: String,
    #[serde(skip_serializing_if = "is_false")]
    pub primary: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub ctrl: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub alt: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub shift: bool,
    #[serde(rename = "super", skip_serializing_if = "is_false")]
    pub super_key: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub physical: bool,
}

const fn is_false(value: &bool) -> bool {
    !*value
}

impl Stroke {
    pub fn key(key: Key) -> Self {
        Self {
            key: key.name().to_owned(),
            ..Self::default()
        }
    }

    pub fn primary(key: Key) -> Self {
        Self {
            primary: true,
            ..Self::key(key)
        }
    }
    pub fn ctrl(key: Key) -> Self {
        Self {
            ctrl: true,
            ..Self::key(key)
        }
    }
    pub fn shift(key: Key) -> Self {
        Self {
            shift: true,
            ..Self::key(key)
        }
    }
    pub fn alt(key: Key) -> Self {
        Self {
            alt: true,
            ..Self::key(key)
        }
    }

    pub fn with_shift(mut self) -> Self {
        self.shift = true;
        self
    }

    pub fn parsed_key(&self) -> Option<Key> {
        Key::from_name(&self.key)
    }

    pub fn label(&self, platform: Platform) -> String {
        let mut parts = Vec::<String>::new();
        if self.primary {
            parts.push(
                if platform == Platform::Macos {
                    "Cmd"
                } else {
                    "Ctrl"
                }
                .into(),
            );
        }
        if self.ctrl {
            parts.push("Ctrl".into());
        }
        if self.alt {
            parts.push(
                if platform == Platform::Macos {
                    "Option"
                } else {
                    "Alt"
                }
                .into(),
            );
        }
        if self.shift {
            parts.push("Shift".into());
        }
        if self.super_key {
            parts.push(
                if platform == Platform::Macos {
                    "Cmd"
                } else {
                    "Super"
                }
                .into(),
            );
        }
        parts.push(
            self.parsed_key()
                .map_or_else(|| self.key.clone(), |key| key.name().to_owned()),
        );
        let mut label = parts.join("+");
        if self.physical {
            label.push_str(" [physical]");
        }
        label
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct BindingRule {
    pub sequence: Vec<Stroke>,
    pub command: String,
    pub scope: Scope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<Platform>,
}

impl BindingRule {
    pub fn new(command: Command, scope: Scope, sequence: Vec<Stroke>) -> Self {
        Self {
            sequence,
            command: command.id().to_owned(),
            scope,
            platform: None,
        }
    }

    pub fn label(&self, platform: Platform) -> String {
        self.sequence
            .iter()
            .map(|stroke| stroke.label(platform))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CustomProfile {
    pub name: String,
    pub behavior: Behavior,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bindings: Vec<BindingRule>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct KeybindingSettings {
    #[serde(skip_serializing_if = "is_vscode")]
    pub active_profile: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub profiles: BTreeMap<String, CustomProfile>,
}

fn is_vscode(value: &String) -> bool {
    value == BUILTIN_VSCODE
}

impl Default for KeybindingSettings {
    fn default() -> Self {
        Self {
            active_profile: BUILTIN_VSCODE.to_owned(),
            profiles: BTreeMap::new(),
        }
    }
}

impl KeybindingSettings {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }

    pub fn profile_behavior(&self, id: &str) -> Option<Behavior> {
        match id {
            BUILTIN_VSCODE => Some(Behavior::Standard),
            BUILTIN_VIM => Some(Behavior::Vim),
            _ => self.profiles.get(id).map(|profile| profile.behavior),
        }
    }

    pub fn active_behavior(&self) -> Behavior {
        self.profile_behavior(&self.active_profile)
            .unwrap_or_default()
    }

    pub fn create_profile(
        &mut self,
        name: &str,
        base: Option<&str>,
        behavior: Behavior,
    ) -> Result<String, String> {
        if self.profiles.len() >= MAX_CUSTOM_PROFILES {
            return Err(format!(
                "at most {MAX_CUSTOM_PROFILES} custom profiles are allowed"
            ));
        }
        validate_profile_name(
            name,
            self.profiles.values().map(|profile| profile.name.as_str()),
        )?;
        if let Some(base) = base {
            let expected =
                builtin_behavior(base).ok_or_else(|| format!("unknown built-in base {base}"))?;
            if expected != behavior {
                return Err(format!(
                    "{base} profiles must use {} editing",
                    expected.label()
                ));
            }
        }
        let id = (1..)
            .map(|number| format!("profile-{number}"))
            .find(|id| !self.profiles.contains_key(id))
            .expect("an unused bounded profile ID exists");
        self.profiles.insert(
            id.clone(),
            CustomProfile {
                name: name.trim().to_owned(),
                behavior,
                base: base.map(str::to_owned),
                removed: Vec::new(),
                bindings: Vec::new(),
            },
        );
        Ok(id)
    }

    pub fn derive_profile(&mut self, base: &str) -> Result<String, String> {
        let behavior =
            builtin_behavior(base).ok_or_else(|| format!("unknown built-in base {base}"))?;
        let label = if base == BUILTIN_VIM {
            "Vim — Custom"
        } else {
            "VS Code — Custom"
        };
        let name = unique_profile_name(
            label,
            self.profiles.values().map(|profile| profile.name.as_str()),
        );
        let id = self.create_profile(&name, Some(base), behavior)?;
        self.active_profile.clone_from(&id);
        Ok(id)
    }

    pub fn duplicate_profile(&mut self, source: &str) -> Result<String, String> {
        let (name, behavior, base, removed, bindings) = match source {
            BUILTIN_VSCODE => (
                "VS Code Copy".to_owned(),
                Behavior::Standard,
                Some(BUILTIN_VSCODE.to_owned()),
                Vec::new(),
                Vec::new(),
            ),
            BUILTIN_VIM => (
                "Vim Copy".to_owned(),
                Behavior::Vim,
                Some(BUILTIN_VIM.to_owned()),
                Vec::new(),
                Vec::new(),
            ),
            _ => {
                let profile = self
                    .profiles
                    .get(source)
                    .ok_or_else(|| format!("unknown profile {source}"))?;
                (
                    format!("{} Copy", profile.name),
                    profile.behavior,
                    profile.base.clone(),
                    profile.removed.clone(),
                    profile.bindings.clone(),
                )
            }
        };
        let name = unique_profile_name(
            &name,
            self.profiles.values().map(|profile| profile.name.as_str()),
        );
        if self.deviation_count() + removed.len() + bindings.len() > MAX_CUSTOM_DEVIATIONS {
            return Err(format!(
                "at most {MAX_CUSTOM_DEVIATIONS} custom bindings and removals are allowed"
            ));
        }
        let id = self.create_profile(&name, base.as_deref(), behavior)?;
        let profile = self.profiles.get_mut(&id).expect("created profile exists");
        profile.removed = removed;
        profile.bindings = bindings;
        self.active_profile.clone_from(&id);
        Ok(id)
    }

    pub fn rename_profile(&mut self, id: &str, name: &str) -> Result<(), String> {
        if !self.profiles.contains_key(id) {
            return Err("built-in profiles cannot be renamed".into());
        }
        validate_profile_name(
            name,
            self.profiles
                .iter()
                .filter(|(candidate, _)| candidate.as_str() != id)
                .map(|(_, profile)| profile.name.as_str()),
        )?;
        self.profiles.get_mut(id).expect("checked above").name = name.trim().to_owned();
        Ok(())
    }

    pub fn delete_profile(&mut self, id: &str, replacement: &str) -> Result<(), String> {
        if !self.profiles.contains_key(id) {
            return Err("built-in profiles cannot be deleted".into());
        }
        if self.active_profile == id {
            if replacement == id || self.profile_behavior(replacement).is_none() {
                return Err(
                    "choose an existing replacement before deleting the active profile".into(),
                );
            }
            self.active_profile = replacement.to_owned();
        }
        self.profiles.remove(id);
        Ok(())
    }

    pub fn set_active(&mut self, id: &str) -> Result<(), String> {
        if self.profile_behavior(id).is_none() {
            return Err(format!("unknown profile {id}"));
        }
        self.active_profile = id.to_owned();
        Ok(())
    }

    pub fn add_binding(
        &mut self,
        profile: &str,
        rule: BindingRule,
        replace_existing: bool,
    ) -> Result<(), String> {
        validate_rule(&rule)?;
        if self.deviation_count() >= MAX_CUSTOM_DEVIATIONS
            && !self.profiles.get(profile).is_some_and(|profile| {
                profile
                    .bindings
                    .iter()
                    .any(|existing| rules_conflict(existing, &rule))
            })
        {
            return Err(format!(
                "at most {MAX_CUSTOM_DEVIATIONS} custom bindings and removals are allowed"
            ));
        }
        let profile = self
            .profiles
            .get_mut(profile)
            .ok_or_else(|| "customize a built-in profile before editing it".to_owned())?;
        let previous = profile.bindings.clone();
        if let Some(index) = profile
            .bindings
            .iter()
            .position(|existing| rules_conflict(existing, &rule))
        {
            if !replace_existing {
                return Err(format!(
                    "{} already uses {}",
                    profile.bindings[index].command,
                    rule.label(Platform::current())
                ));
            }
            profile.bindings.remove(index);
        }
        profile.bindings.push(rule);
        if let Err(error) = validate_profile_rules(profile) {
            profile.bindings = previous;
            return Err(error);
        }
        Ok(())
    }

    pub fn remove_binding(&mut self, profile: &str, index: usize) -> Result<(), String> {
        let bindings = &mut self
            .profiles
            .get_mut(profile)
            .ok_or_else(|| "built-in profiles are read-only".to_owned())?
            .bindings;
        if index >= bindings.len() {
            return Err("binding no longer exists".into());
        }
        bindings.remove(index);
        Ok(())
    }

    pub fn disable_binding(&mut self, profile: &str, binding_id: &str) -> Result<(), String> {
        if self.deviation_count() >= MAX_CUSTOM_DEVIATIONS
            && !self
                .profiles
                .get(profile)
                .is_some_and(|profile| profile.removed.iter().any(|id| id == binding_id))
        {
            return Err(format!(
                "at most {MAX_CUSTOM_DEVIATIONS} custom bindings and removals are allowed"
            ));
        }
        let custom = self
            .profiles
            .get_mut(profile)
            .ok_or_else(|| "built-in profiles are read-only".to_owned())?;
        let base = custom
            .base
            .as_deref()
            .ok_or_else(|| "an empty profile has no inherited bindings".to_owned())?;
        if !builtin_bindings(base)
            .is_some_and(|rules| rules.iter().any(|binding| binding.id == binding_id))
        {
            return Err(format!("unknown inherited binding {binding_id}"));
        }
        if !custom.removed.iter().any(|id| id == binding_id) {
            custom.removed.push(binding_id.to_owned());
        }
        Ok(())
    }

    pub fn reset_command(&mut self, profile: &str, command: Command) -> Result<(), String> {
        let custom = self
            .profiles
            .get_mut(profile)
            .ok_or_else(|| "built-in profiles are read-only".to_owned())?;
        custom
            .bindings
            .retain(|binding| binding.command != command.id());
        if let Some(base) = custom.base.as_deref() {
            let restored = builtin_bindings(base)
                .unwrap_or_default()
                .into_iter()
                .filter(|binding| binding.rule.command == command.id())
                .map(|binding| binding.id)
                .collect::<HashSet<_>>();
            custom.removed.retain(|binding| !restored.contains(binding));
        }
        Ok(())
    }

    pub fn reset_all(&mut self, profile: &str) -> Result<(), String> {
        let custom = self
            .profiles
            .get_mut(profile)
            .ok_or_else(|| "built-in profiles are read-only".to_owned())?;
        custom.bindings.clear();
        custom.removed.clear();
        Ok(())
    }

    pub fn effective_bindings(&self) -> Result<Vec<EffectiveBinding>, String> {
        self.effective_bindings_for(&self.active_profile)
    }

    pub fn effective_bindings_for(&self, profile: &str) -> Result<Vec<EffectiveBinding>, String> {
        if let Some(bindings) = builtin_bindings(profile) {
            return Ok(bindings
                .into_iter()
                .map(EffectiveBinding::builtin)
                .collect());
        }
        let profile = self
            .profiles
            .get(profile)
            .ok_or_else(|| format!("unknown profile {profile}"))?;
        let mut effective = profile
            .bindings
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, rule)| EffectiveBinding {
                id: format!("custom-{index}"),
                source: BindingSource::Custom,
                rule,
            })
            .collect::<Vec<_>>();
        if let Some(base) = profile.base.as_deref() {
            let removed = profile
                .removed
                .iter()
                .map(String::as_str)
                .collect::<HashSet<_>>();
            effective.extend(
                builtin_bindings(base)
                    .ok_or_else(|| format!("unknown built-in base {base}"))?
                    .into_iter()
                    .filter(|binding| !removed.contains(binding.id.as_str()))
                    .map(EffectiveBinding::builtin),
            );
        }
        Ok(effective)
    }

    fn deviation_count(&self) -> usize {
        self.profiles
            .values()
            .map(|profile| profile.bindings.len() + profile.removed.len())
            .sum()
    }
}

fn builtin_behavior(profile: &str) -> Option<Behavior> {
    match profile {
        BUILTIN_VSCODE => Some(Behavior::Standard),
        BUILTIN_VIM => Some(Behavior::Vim),
        _ => None,
    }
}

fn validate_profile_name<'a>(
    name: &str,
    mut existing: impl Iterator<Item = &'a str>,
) -> Result<(), String> {
    let name = name.trim();
    if !(1..=64).contains(&name.chars().count()) {
        return Err("profile names must contain 1 to 64 characters".into());
    }
    let folded = name.to_lowercase();
    if existing.any(|candidate| candidate.to_lowercase() == folded) {
        return Err(format!("a profile named {name} already exists"));
    }
    Ok(())
}

fn unique_profile_name<'a>(preferred: &str, existing: impl Iterator<Item = &'a str>) -> String {
    let existing = existing
        .map(str::to_ascii_lowercase)
        .collect::<HashSet<_>>();
    if !existing.contains(&preferred.to_ascii_lowercase()) {
        return preferred.to_owned();
    }
    (2..)
        .map(|number| format!("{preferred} {number}"))
        .find(|name| !existing.contains(&name.to_ascii_lowercase()))
        .expect("bounded profiles leave a name")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BindingSource {
    BuiltIn,
    Custom,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectiveBinding {
    pub id: String,
    pub source: BindingSource,
    pub rule: BindingRule,
}

impl EffectiveBinding {
    fn builtin(binding: BuiltinBinding) -> Self {
        Self {
            id: binding.id,
            source: BindingSource::BuiltIn,
            rule: binding.rule,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct InputStroke {
    pub key: Key,
    pub physical_key: Option<Key>,
    pub modifiers: Modifiers,
}

impl InputStroke {
    pub const fn new(key: Key, physical_key: Option<Key>, modifiers: Modifiers) -> Self {
        Self {
            key,
            physical_key,
            modifiers,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ResolveResult {
    pub command: Option<Command>,
    pub consumed: bool,
    pub reprocessed: bool,
}

pub struct Resolver {
    rules: Vec<EffectiveBinding>,
    platform: Platform,
    pending: Vec<InputStroke>,
    pending_deadline: Option<Duration>,
}

impl Resolver {
    pub fn new(rules: Vec<EffectiveBinding>, platform: Platform) -> Result<Self, String> {
        for binding in &rules {
            validate_rule(&binding.rule)?;
        }
        validate_effective_sequences(&rules)?;
        Ok(Self {
            rules,
            platform,
            pending: Vec::new(),
            pending_deadline: None,
        })
    }

    pub fn clear_pending(&mut self) {
        self.pending.clear();
        self.pending_deadline = None;
    }

    pub fn pending_label(&self) -> Option<String> {
        (!self.pending.is_empty()).then(|| {
            self.pending
                .iter()
                .map(|stroke| input_label(*stroke, self.platform))
                .collect::<Vec<_>>()
                .join(" ")
        })
    }

    pub fn resolve(
        &mut self,
        input: InputStroke,
        scopes: &[Scope],
        repeated: bool,
        now: Duration,
    ) -> ResolveResult {
        if self
            .pending_deadline
            .is_some_and(|deadline| now >= deadline)
        {
            self.clear_pending();
        }
        if !self.pending.is_empty() && input.key == Key::Escape {
            self.clear_pending();
            return ResolveResult {
                consumed: true,
                ..ResolveResult::default()
            };
        }
        let had_pending = !self.pending.is_empty();
        self.pending.push(input);
        let result = self.resolve_pending(scopes, repeated, now);
        if result.consumed || !had_pending {
            return result;
        }
        self.clear_pending();
        self.pending.push(input);
        let mut result = self.resolve_pending(scopes, repeated, now);
        result.reprocessed = true;
        result
    }

    fn resolve_pending(
        &mut self,
        scopes: &[Scope],
        repeated: bool,
        now: Duration,
    ) -> ResolveResult {
        let mut matches = self
            .rules
            .iter()
            .filter(|binding| {
                binding
                    .rule
                    .platform
                    .is_none_or(|platform| platform == self.platform)
                    && (binding.rule.scope == Scope::Global || scopes.contains(&binding.rule.scope))
                    && !(binding.source == BindingSource::BuiltIn
                        && binding.rule.scope == Scope::Global
                        && scopes.contains(&Scope::Terminal)
                        && binding
                            .rule
                            .sequence
                            .first()
                            .is_some_and(|stroke| terminal_control(stroke, self.platform)))
                    && self.pending.len() <= binding.rule.sequence.len()
                    && binding
                        .rule
                        .sequence
                        .iter()
                        .zip(&self.pending)
                        .all(|(expected, actual)| expected.matches(*actual, self.platform))
            })
            .collect::<Vec<_>>();
        if matches.is_empty() {
            return ResolveResult::default();
        }
        matches.sort_by_key(|binding| rule_rank(binding, scopes));
        if let Some(binding) = matches
            .iter()
            .find(|binding| binding.rule.sequence.len() == self.pending.len())
            .copied()
        {
            let command = Command::from_id(&binding.rule.command).expect("rules were validated");
            self.clear_pending();
            return ResolveResult {
                command: (!repeated || command.info().repeatable).then_some(command),
                consumed: true,
                reprocessed: false,
            };
        }
        self.pending_deadline = Some(now + Duration::from_secs(1));
        ResolveResult {
            consumed: true,
            ..ResolveResult::default()
        }
    }
}

fn terminal_control(stroke: &Stroke, platform: Platform) -> bool {
    let ctrl = stroke.ctrl || (stroke.primary && platform != Platform::Macos);
    ctrl && !stroke.alt
        && !stroke.shift
        && !stroke.super_key
        && stroke.parsed_key().is_some_and(|key| {
            matches!(
                key,
                Key::A
                    | Key::B
                    | Key::C
                    | Key::D
                    | Key::E
                    | Key::F
                    | Key::G
                    | Key::H
                    | Key::I
                    | Key::J
                    | Key::K
                    | Key::L
                    | Key::M
                    | Key::N
                    | Key::O
                    | Key::P
                    | Key::Q
                    | Key::R
                    | Key::S
                    | Key::T
                    | Key::U
                    | Key::V
                    | Key::W
                    | Key::X
                    | Key::Y
                    | Key::Z
                    | Key::OpenBracket
                    | Key::Backslash
                    | Key::CloseBracket
            )
        })
}

fn rule_rank(binding: &EffectiveBinding, scopes: &[Scope]) -> (u8, usize) {
    let source = match binding.source {
        BindingSource::Custom => 0,
        BindingSource::BuiltIn => 1,
    };
    let scope = if binding.rule.scope == Scope::Global {
        usize::MAX
    } else {
        scopes
            .iter()
            .position(|scope| *scope == binding.rule.scope)
            .unwrap_or(usize::MAX - 1)
    };
    (source, scope)
}

impl Stroke {
    fn matches(&self, input: InputStroke, platform: Platform) -> bool {
        let Some(key) = self.parsed_key() else {
            return false;
        };
        let actual_key = if self.physical {
            input.physical_key
        } else {
            Some(input.key)
        };
        if actual_key != Some(key) {
            return false;
        }
        if !self.physical && input.modifiers.ctrl && input.modifiers.alt && printable(input.key) {
            return false;
        }
        let primary_ctrl = self.primary && platform != Platform::Macos;
        let primary_mac = self.primary && platform == Platform::Macos;
        let actual_ctrl =
            input.modifiers.ctrl || (platform != Platform::Macos && input.modifiers.command);
        let actual_mac =
            input.modifiers.mac_cmd || (platform == Platform::Macos && input.modifiers.command);
        actual_ctrl == (self.ctrl || primary_ctrl)
            && input.modifiers.alt == self.alt
            && input.modifiers.shift == self.shift
            && actual_mac == (self.super_key || primary_mac)
    }
}

fn input_label(input: InputStroke, platform: Platform) -> String {
    Stroke {
        key: input.key.name().to_owned(),
        ctrl: input.modifiers.ctrl,
        alt: input.modifiers.alt,
        shift: input.modifiers.shift,
        super_key: input.modifiers.mac_cmd,
        ..Stroke::default()
    }
    .label(platform)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuiltinBinding {
    pub id: String,
    pub rule: BindingRule,
}

fn builtin(
    id: impl Into<String>,
    command: Command,
    scope: Scope,
    sequence: Vec<Stroke>,
) -> BuiltinBinding {
    BuiltinBinding {
        id: id.into(),
        rule: BindingRule::new(command, scope, sequence),
    }
}

fn platform_builtin(
    id: impl Into<String>,
    command: Command,
    scope: Scope,
    platform: Platform,
    sequence: Vec<Stroke>,
) -> BuiltinBinding {
    let mut binding = builtin(id, command, scope, sequence);
    binding.rule.platform = Some(platform);
    binding
}

pub fn vscode_bindings() -> Vec<BuiltinBinding> {
    let mut rules = vec![
        builtin(
            "vscode.file.save",
            Command::FileSave,
            Scope::Global,
            vec![Stroke::primary(Key::S)],
        ),
        builtin(
            "vscode.search.find",
            Command::SearchFind,
            Scope::Global,
            vec![Stroke::primary(Key::F)],
        ),
        builtin(
            "vscode.search.project",
            Command::SearchProject,
            Scope::Global,
            vec![Stroke::primary(Key::F).with_shift()],
        ),
        builtin(
            "vscode.view.sidebar",
            Command::ViewToggleSidebar,
            Scope::Global,
            vec![Stroke::primary(Key::B)],
        ),
        builtin(
            "vscode.view.explorer",
            Command::ViewFocusExplorer,
            Scope::Global,
            vec![Stroke::primary(Key::E).with_shift()],
        ),
        builtin(
            "vscode.file.split",
            Command::FileSplitEditor,
            Scope::Global,
            vec![Stroke::primary(Key::Backslash)],
        ),
        builtin(
            "vscode.view.terminal",
            Command::ViewToggleTerminal,
            Scope::Global,
            vec![Stroke::ctrl(Key::Backtick)],
        ),
        builtin(
            "vscode.view.markdown",
            Command::ViewToggleMarkdownPreview,
            Scope::Global,
            vec![Stroke::primary(Key::V).with_shift()],
        ),
        builtin(
            "vscode.application.settings",
            Command::AppOpenSettings,
            Scope::Global,
            vec![Stroke::primary(Key::Comma)],
        ),
        builtin(
            "vscode.application.keybindings",
            Command::AppOpenKeybindings,
            Scope::Global,
            vec![Stroke::primary(Key::K), Stroke::primary(Key::S)],
        ),
        builtin(
            "vscode.application.increaseUiScale",
            Command::AppIncreaseUiScale,
            Scope::Global,
            vec![Stroke::primary(Key::Plus).with_shift()],
        ),
        builtin(
            "vscode.application.decreaseUiScale",
            Command::AppDecreaseUiScale,
            Scope::Global,
            vec![Stroke::primary(Key::Minus).with_shift()],
        ),
        builtin(
            "vscode.editor.copy",
            Command::EditorCopy,
            Scope::DocumentEditor,
            vec![Stroke::primary(Key::C)],
        ),
        builtin(
            "vscode.editor.cut",
            Command::EditorCut,
            Scope::DocumentEditor,
            vec![Stroke::primary(Key::X)],
        ),
        builtin(
            "vscode.editor.paste",
            Command::EditorPaste,
            Scope::DocumentEditor,
            vec![Stroke::primary(Key::V)],
        ),
        builtin(
            "vscode.editor.selectAll",
            Command::EditorSelectAll,
            Scope::DocumentEditor,
            vec![Stroke::primary(Key::A)],
        ),
        builtin(
            "vscode.editor.deleteLeft",
            Command::EditorDeleteLeft,
            Scope::DocumentEditor,
            vec![Stroke::key(Key::Backspace)],
        ),
        builtin(
            "vscode.editor.deleteRight",
            Command::EditorDeleteRight,
            Scope::DocumentEditor,
            vec![Stroke::key(Key::Delete)],
        ),
        builtin(
            "vscode.editor.lineBreak",
            Command::EditorInsertLineBreak,
            Scope::DocumentEditor,
            vec![Stroke::key(Key::Enter)],
        ),
        builtin(
            "vscode.editor.indent",
            Command::EditorIndent,
            Scope::DocumentEditor,
            vec![Stroke::key(Key::Tab)],
        ),
        builtin(
            "vscode.editor.outdent",
            Command::EditorOutdent,
            Scope::DocumentEditor,
            vec![Stroke::shift(Key::Tab)],
        ),
        builtin(
            "vscode.editor.left",
            Command::EditorCursorLeft,
            Scope::DocumentEditor,
            vec![Stroke::key(Key::ArrowLeft)],
        ),
        builtin(
            "vscode.editor.right",
            Command::EditorCursorRight,
            Scope::DocumentEditor,
            vec![Stroke::key(Key::ArrowRight)],
        ),
        builtin(
            "vscode.editor.up",
            Command::EditorCursorUp,
            Scope::DocumentEditor,
            vec![Stroke::key(Key::ArrowUp)],
        ),
        builtin(
            "vscode.editor.down",
            Command::EditorCursorDown,
            Scope::DocumentEditor,
            vec![Stroke::key(Key::ArrowDown)],
        ),
        builtin(
            "vscode.editor.selectLeft",
            Command::EditorSelectLeft,
            Scope::DocumentEditor,
            vec![Stroke::shift(Key::ArrowLeft)],
        ),
        builtin(
            "vscode.editor.selectRight",
            Command::EditorSelectRight,
            Scope::DocumentEditor,
            vec![Stroke::shift(Key::ArrowRight)],
        ),
        builtin(
            "vscode.editor.selectUp",
            Command::EditorSelectUp,
            Scope::DocumentEditor,
            vec![Stroke::shift(Key::ArrowUp)],
        ),
        builtin(
            "vscode.editor.selectDown",
            Command::EditorSelectDown,
            Scope::DocumentEditor,
            vec![Stroke::shift(Key::ArrowDown)],
        ),
        builtin(
            "vscode.editor.home",
            Command::EditorCursorLineStart,
            Scope::DocumentEditor,
            vec![Stroke::key(Key::Home)],
        ),
        builtin(
            "vscode.editor.end",
            Command::EditorCursorLineEnd,
            Scope::DocumentEditor,
            vec![Stroke::key(Key::End)],
        ),
        builtin(
            "vscode.editor.pageUp",
            Command::EditorCursorPageUp,
            Scope::DocumentEditor,
            vec![Stroke::key(Key::PageUp)],
        ),
        builtin(
            "vscode.editor.pageDown",
            Command::EditorCursorPageDown,
            Scope::DocumentEditor,
            vec![Stroke::key(Key::PageDown)],
        ),
        builtin(
            "vscode.tree.up",
            Command::TreeMoveUp,
            Scope::FilesTree,
            vec![Stroke::key(Key::ArrowUp)],
        ),
        builtin(
            "vscode.tree.down",
            Command::TreeMoveDown,
            Scope::FilesTree,
            vec![Stroke::key(Key::ArrowDown)],
        ),
        builtin(
            "vscode.tree.expand",
            Command::TreeExpand,
            Scope::FilesTree,
            vec![Stroke::key(Key::ArrowRight)],
        ),
        builtin(
            "vscode.tree.collapse",
            Command::TreeCollapse,
            Scope::FilesTree,
            vec![Stroke::key(Key::ArrowLeft)],
        ),
        builtin(
            "vscode.tree.open",
            Command::TreeOpen,
            Scope::FilesTree,
            vec![Stroke::key(Key::Enter)],
        ),
        builtin(
            "vscode.editor.suggest",
            Command::EditorTriggerSuggest,
            Scope::DocumentEditor,
            vec![Stroke::ctrl(Key::Space)],
        ),
        builtin(
            "vscode.editor.definition",
            Command::EditorGoToDefinition,
            Scope::DocumentEditor,
            vec![Stroke::key(Key::F12)],
        ),
        builtin(
            "vscode.editor.diagnosticNext",
            Command::EditorNextDiagnostic,
            Scope::DocumentEditor,
            vec![Stroke::key(Key::F8)],
        ),
        builtin(
            "vscode.editor.diagnosticPrevious",
            Command::EditorPreviousDiagnostic,
            Scope::DocumentEditor,
            vec![Stroke::shift(Key::F8)],
        ),
    ];
    for (index, command) in [
        Command::FileFocusPane1,
        Command::FileFocusPane2,
        Command::FileFocusPane3,
        Command::FileFocusPane4,
        Command::FileFocusPane5,
        Command::FileFocusPane6,
        Command::FileFocusPane7,
        Command::FileFocusPane8,
        Command::FileFocusPane9,
    ]
    .into_iter()
    .enumerate()
    {
        let key = [
            Key::Num1,
            Key::Num2,
            Key::Num3,
            Key::Num4,
            Key::Num5,
            Key::Num6,
            Key::Num7,
            Key::Num8,
            Key::Num9,
        ][index];
        rules.push(builtin(
            format!("vscode.file.focusPane{}", index + 1),
            command,
            Scope::Global,
            vec![Stroke::primary(key)],
        ));
    }
    rules.extend([
        platform_builtin(
            "vscode.application.close.macos",
            Command::AppCloseWindow,
            Scope::Global,
            Platform::Macos,
            vec![Stroke::primary(Key::Q)],
        ),
        platform_builtin(
            "vscode.application.close.windows",
            Command::AppCloseWindow,
            Scope::Global,
            Platform::Windows,
            vec![Stroke::alt(Key::F4)],
        ),
        platform_builtin(
            "vscode.application.close.linux",
            Command::AppCloseWindow,
            Scope::Global,
            Platform::Linux,
            vec![Stroke::alt(Key::F4)],
        ),
        platform_builtin(
            "vscode.file.close.macos",
            Command::FileCloseActive,
            Scope::Global,
            Platform::Macos,
            vec![Stroke::primary(Key::W)],
        ),
        platform_builtin(
            "vscode.file.close.windows",
            Command::FileCloseActive,
            Scope::Global,
            Platform::Windows,
            vec![Stroke::ctrl(Key::F4)],
        ),
        platform_builtin(
            "vscode.file.close.linux",
            Command::FileCloseActive,
            Scope::Global,
            Platform::Linux,
            vec![Stroke::ctrl(Key::W)],
        ),
        platform_builtin(
            "vscode.editor.undo.macos",
            Command::EditorUndo,
            Scope::DocumentEditor,
            Platform::Macos,
            vec![Stroke::primary(Key::Z)],
        ),
        platform_builtin(
            "vscode.editor.undo.windows",
            Command::EditorUndo,
            Scope::DocumentEditor,
            Platform::Windows,
            vec![Stroke::ctrl(Key::Z)],
        ),
        platform_builtin(
            "vscode.editor.undo.linux",
            Command::EditorUndo,
            Scope::DocumentEditor,
            Platform::Linux,
            vec![Stroke::ctrl(Key::Z)],
        ),
        platform_builtin(
            "vscode.editor.redo.macos",
            Command::EditorRedo,
            Scope::DocumentEditor,
            Platform::Macos,
            vec![Stroke::primary(Key::Z).with_shift()],
        ),
        platform_builtin(
            "vscode.editor.redo.windows",
            Command::EditorRedo,
            Scope::DocumentEditor,
            Platform::Windows,
            vec![Stroke::ctrl(Key::Y)],
        ),
        platform_builtin(
            "vscode.editor.redo.linux",
            Command::EditorRedo,
            Scope::DocumentEditor,
            Platform::Linux,
            vec![Stroke::ctrl(Key::Y)],
        ),
    ]);
    for platform in [Platform::Macos, Platform::Windows, Platform::Linux] {
        let word_modifier = if platform == Platform::Macos {
            Stroke::alt
        } else {
            Stroke::ctrl
        };
        for (id, command, key, shift) in [
            (
                "wordLeft",
                Command::EditorCursorWordLeft,
                Key::ArrowLeft,
                false,
            ),
            (
                "wordRight",
                Command::EditorCursorWordRight,
                Key::ArrowRight,
                false,
            ),
            (
                "selectWordLeft",
                Command::EditorSelectWordLeft,
                Key::ArrowLeft,
                true,
            ),
            (
                "selectWordRight",
                Command::EditorSelectWordRight,
                Key::ArrowRight,
                true,
            ),
        ] {
            let mut stroke = word_modifier(key);
            stroke.shift = shift;
            rules.push(platform_builtin(
                format!("vscode.editor.{id}.{platform:?}"),
                command,
                Scope::DocumentEditor,
                platform,
                vec![stroke],
            ));
        }
    }
    rules
}

pub fn vim_bindings() -> Vec<BuiltinBinding> {
    let mut rules = vscode_bindings()
        .into_iter()
        .filter(|binding| {
            binding.rule.scope == Scope::Global && binding.id != "vscode.file.close.linux"
        })
        .map(|mut binding| {
            binding.id = format!("vim.base.{}", binding.id);
            binding
        })
        .collect::<Vec<_>>();
    let normal = Scope::VimNormal;
    let visual = Scope::VimVisual;
    let operator = Scope::VimOperator;
    let mappings = [
        ("normal.i", Command::VimInsert, normal, Key::I, false, false),
        (
            "normal.I",
            Command::VimInsertLineStart,
            normal,
            Key::I,
            true,
            false,
        ),
        ("normal.a", Command::VimAppend, normal, Key::A, false, false),
        (
            "normal.A",
            Command::VimAppendLineEnd,
            normal,
            Key::A,
            true,
            false,
        ),
        (
            "normal.o",
            Command::VimOpenBelow,
            normal,
            Key::O,
            false,
            false,
        ),
        (
            "normal.O",
            Command::VimOpenAbove,
            normal,
            Key::O,
            true,
            false,
        ),
        (
            "normal.s",
            Command::VimSubstituteChar,
            normal,
            Key::S,
            false,
            false,
        ),
        (
            "normal.S",
            Command::VimSubstituteLine,
            normal,
            Key::S,
            true,
            false,
        ),
        (
            "normal.C",
            Command::VimChangeLineEnd,
            normal,
            Key::C,
            true,
            false,
        ),
        (
            "normal.R",
            Command::VimReplaceMode,
            normal,
            Key::R,
            true,
            false,
        ),
        (
            "normal.v",
            Command::VimVisualCharacter,
            normal,
            Key::V,
            false,
            false,
        ),
        (
            "normal.V",
            Command::VimVisualLine,
            normal,
            Key::V,
            true,
            false,
        ),
        (
            "motion.h",
            Command::VimMoveLeft,
            normal,
            Key::H,
            false,
            false,
        ),
        (
            "motion.j",
            Command::VimMoveDown,
            normal,
            Key::J,
            false,
            false,
        ),
        ("motion.k", Command::VimMoveUp, normal, Key::K, false, false),
        (
            "motion.l",
            Command::VimMoveRight,
            normal,
            Key::L,
            false,
            false,
        ),
        (
            "motion.0",
            Command::VimCount0,
            normal,
            Key::Num0,
            false,
            false,
        ),
        (
            "motion.caret",
            Command::VimFirstNonBlank,
            normal,
            Key::Num6,
            true,
            false,
        ),
        (
            "motion.dollar",
            Command::VimLineEnd,
            normal,
            Key::Num4,
            true,
            false,
        ),
        ("motion.G", Command::VimFileEnd, normal, Key::G, true, false),
        (
            "motion.w",
            Command::VimWordForward,
            normal,
            Key::W,
            false,
            false,
        ),
        (
            "motion.W",
            Command::VimBigWordForward,
            normal,
            Key::W,
            true,
            false,
        ),
        (
            "motion.e",
            Command::VimWordEnd,
            normal,
            Key::E,
            false,
            false,
        ),
        (
            "motion.E",
            Command::VimBigWordEnd,
            normal,
            Key::E,
            true,
            false,
        ),
        (
            "motion.b",
            Command::VimWordBack,
            normal,
            Key::B,
            false,
            false,
        ),
        (
            "motion.B",
            Command::VimBigWordBack,
            normal,
            Key::B,
            true,
            false,
        ),
        (
            "motion.paragraphBack",
            Command::VimParagraphBack,
            normal,
            Key::OpenCurlyBracket,
            true,
            false,
        ),
        (
            "motion.paragraphForward",
            Command::VimParagraphForward,
            normal,
            Key::CloseCurlyBracket,
            true,
            false,
        ),
        (
            "motion.f",
            Command::VimFindForward,
            normal,
            Key::F,
            false,
            false,
        ),
        (
            "motion.F",
            Command::VimFindBackward,
            normal,
            Key::F,
            true,
            false,
        ),
        (
            "motion.t",
            Command::VimTillForward,
            normal,
            Key::T,
            false,
            false,
        ),
        (
            "motion.T",
            Command::VimTillBackward,
            normal,
            Key::T,
            true,
            false,
        ),
        (
            "motion.repeatFind",
            Command::VimRepeatFind,
            normal,
            Key::Semicolon,
            false,
            false,
        ),
        (
            "motion.reverseFind",
            Command::VimReverseFind,
            normal,
            Key::Comma,
            false,
            false,
        ),
        (
            "operator.d",
            Command::VimOperatorDelete,
            normal,
            Key::D,
            false,
            false,
        ),
        (
            "operator.c",
            Command::VimOperatorChange,
            normal,
            Key::C,
            false,
            false,
        ),
        (
            "operator.y",
            Command::VimOperatorYank,
            normal,
            Key::Y,
            false,
            false,
        ),
        (
            "operator.indent",
            Command::VimOperatorIndent,
            normal,
            Key::Period,
            true,
            false,
        ),
        (
            "operator.outdent",
            Command::VimOperatorOutdent,
            normal,
            Key::Comma,
            true,
            false,
        ),
        (
            "change.x",
            Command::VimDeleteCharacter,
            normal,
            Key::X,
            false,
            false,
        ),
        (
            "change.X",
            Command::VimDeleteCharacterLeft,
            normal,
            Key::X,
            true,
            false,
        ),
        (
            "change.D",
            Command::VimDeleteLineEnd,
            normal,
            Key::D,
            true,
            false,
        ),
        (
            "change.r",
            Command::VimReplaceCharacter,
            normal,
            Key::R,
            false,
            false,
        ),
        (
            "change.J",
            Command::VimJoinLines,
            normal,
            Key::J,
            true,
            false,
        ),
        (
            "change.toggleCase",
            Command::VimToggleCase,
            normal,
            Key::Backtick,
            true,
            false,
        ),
        (
            "change.p",
            Command::VimPutAfter,
            normal,
            Key::P,
            false,
            false,
        ),
        (
            "change.P",
            Command::VimPutBefore,
            normal,
            Key::P,
            true,
            false,
        ),
        ("history.u", Command::VimUndo, normal, Key::U, false, false),
        (
            "history.repeat",
            Command::VimRepeat,
            normal,
            Key::Period,
            false,
            false,
        ),
        (
            "search.forward",
            Command::VimSearchForward,
            normal,
            Key::Slash,
            false,
            false,
        ),
        (
            "search.backward",
            Command::VimSearchBackward,
            normal,
            Key::Questionmark,
            true,
            false,
        ),
        (
            "search.next",
            Command::VimSearchNext,
            normal,
            Key::N,
            false,
            false,
        ),
        (
            "search.previous",
            Command::VimSearchPrevious,
            normal,
            Key::N,
            true,
            false,
        ),
        (
            "search.wordForward",
            Command::VimSearchWordForward,
            normal,
            Key::Num8,
            true,
            false,
        ),
        (
            "search.wordBackward",
            Command::VimSearchWordBackward,
            normal,
            Key::Num3,
            true,
            false,
        ),
        ("ex.open", Command::VimEx, normal, Key::Colon, true, false),
        (
            "operator.inner",
            Command::VimTextInner,
            operator,
            Key::I,
            false,
            false,
        ),
        (
            "operator.around",
            Command::VimTextAround,
            operator,
            Key::A,
            false,
            false,
        ),
        (
            "visual.d",
            Command::VimOperatorDelete,
            visual,
            Key::D,
            false,
            false,
        ),
        (
            "visual.c",
            Command::VimOperatorChange,
            visual,
            Key::C,
            false,
            false,
        ),
        (
            "visual.y",
            Command::VimOperatorYank,
            visual,
            Key::Y,
            false,
            false,
        ),
        (
            "visual.indent",
            Command::VimOperatorIndent,
            visual,
            Key::Period,
            true,
            false,
        ),
        (
            "visual.outdent",
            Command::VimOperatorOutdent,
            visual,
            Key::Comma,
            true,
            false,
        ),
        (
            "visual.toggleCase",
            Command::VimToggleCase,
            visual,
            Key::Backtick,
            true,
            false,
        ),
    ];
    for (id, command, scope, key, shift, ctrl) in mappings {
        let mut stroke = Stroke::key(key);
        stroke.shift = shift;
        stroke.ctrl = ctrl;
        rules.push(builtin(format!("vim.{id}"), command, scope, vec![stroke]));
    }
    let motions = rules
        .iter()
        .filter(|binding| binding.rule.scope == normal && binding.id.starts_with("vim.motion."))
        .cloned()
        .collect::<Vec<_>>();
    for scope in [visual, operator] {
        for mut binding in motions.clone() {
            binding.id = format!("{}.{scope:?}", binding.id);
            binding.rule.scope = scope;
            rules.push(binding);
        }
    }
    for id in [
        "vim.operator.d",
        "vim.operator.c",
        "vim.operator.y",
        "vim.operator.indent",
        "vim.operator.outdent",
    ] {
        let mut binding = rules
            .iter()
            .find(|binding| binding.id == id)
            .expect("operator mapping exists")
            .clone();
        binding.id = format!("{id}.pending");
        binding.rule.scope = operator;
        rules.push(binding);
    }
    for scope in [normal, visual, operator] {
        for (id, command, key) in [
            ("left", Command::VimMoveLeft, Key::ArrowLeft),
            ("right", Command::VimMoveRight, Key::ArrowRight),
            ("up", Command::VimMoveUp, Key::ArrowUp),
            ("down", Command::VimMoveDown, Key::ArrowDown),
            ("home", Command::VimLineStart, Key::Home),
            ("end", Command::VimLineEnd, Key::End),
            ("pageUp", Command::EditorCursorPageUp, Key::PageUp),
            ("pageDown", Command::EditorCursorPageDown, Key::PageDown),
        ] {
            rules.push(builtin(
                format!("vim.{scope:?}.{id}"),
                command,
                scope,
                vec![Stroke::key(key)],
            ));
        }
        for digit in 1..=9 {
            let commands = [
                Command::VimCount1,
                Command::VimCount2,
                Command::VimCount3,
                Command::VimCount4,
                Command::VimCount5,
                Command::VimCount6,
                Command::VimCount7,
                Command::VimCount8,
                Command::VimCount9,
            ];
            let keys = [
                Key::Num1,
                Key::Num2,
                Key::Num3,
                Key::Num4,
                Key::Num5,
                Key::Num6,
                Key::Num7,
                Key::Num8,
                Key::Num9,
            ];
            rules.push(builtin(
                format!("vim.{scope:?}.count{digit}"),
                commands[digit - 1],
                scope,
                vec![Stroke::key(keys[digit - 1])],
            ));
        }
    }
    rules.extend([
        builtin(
            "vim.normal.gg",
            Command::VimFileStart,
            normal,
            vec![Stroke::key(Key::G), Stroke::key(Key::G)],
        ),
        builtin(
            "vim.visual.gg",
            Command::VimFileStart,
            visual,
            vec![Stroke::key(Key::G), Stroke::key(Key::G)],
        ),
        builtin(
            "vim.operator.gg",
            Command::VimFileStart,
            operator,
            vec![Stroke::key(Key::G), Stroke::key(Key::G)],
        ),
        builtin(
            "vim.normal.redo",
            Command::VimRedo,
            normal,
            vec![Stroke::ctrl(Key::R)],
        ),
        builtin(
            "vim.normal.escape",
            Command::VimNormal,
            normal,
            vec![Stroke::key(Key::Escape)],
        ),
        builtin(
            "vim.insert.escape",
            Command::VimNormal,
            Scope::VimInsert,
            vec![Stroke::key(Key::Escape)],
        ),
        builtin(
            "vim.replace.escape",
            Command::VimNormal,
            Scope::VimReplace,
            vec![Stroke::key(Key::Escape)],
        ),
        builtin(
            "vim.visual.escape",
            Command::VimNormal,
            visual,
            vec![Stroke::key(Key::Escape)],
        ),
        builtin(
            "vim.visual.character",
            Command::VimVisualCharacter,
            visual,
            vec![Stroke::key(Key::V)],
        ),
        builtin(
            "vim.visual.line",
            Command::VimVisualLine,
            visual,
            vec![Stroke::shift(Key::V)],
        ),
        builtin(
            "vim.operator.escape",
            Command::VimNormal,
            operator,
            vec![Stroke::key(Key::Escape)],
        ),
        builtin(
            "vim.insert.ctrlBracket",
            Command::VimNormal,
            Scope::VimInsert,
            vec![Stroke::ctrl(Key::OpenBracket)],
        ),
        builtin(
            "vim.replace.ctrlBracket",
            Command::VimNormal,
            Scope::VimReplace,
            vec![Stroke::ctrl(Key::OpenBracket)],
        ),
        builtin(
            "vim.normal.systemRegister",
            Command::VimRegisterSystem,
            normal,
            vec![Stroke::shift(Key::Quote), Stroke::shift(Key::Plus)],
        ),
        builtin(
            "vim.pane.left",
            Command::FileFocusLeftPane,
            normal,
            vec![Stroke::ctrl(Key::W), Stroke::key(Key::H)],
        ),
        builtin(
            "vim.pane.down",
            Command::FileFocusNextPane,
            normal,
            vec![Stroke::ctrl(Key::W), Stroke::key(Key::J)],
        ),
        builtin(
            "vim.pane.up",
            Command::FileFocusPreviousPane,
            normal,
            vec![Stroke::ctrl(Key::W), Stroke::key(Key::K)],
        ),
        builtin(
            "vim.pane.right",
            Command::FileFocusRightPane,
            normal,
            vec![Stroke::ctrl(Key::W), Stroke::key(Key::L)],
        ),
        builtin(
            "vim.pane.next",
            Command::FileFocusNextPane,
            normal,
            vec![Stroke::ctrl(Key::W), Stroke::key(Key::W)],
        ),
        builtin(
            "vim.pane.close",
            Command::FileCloseActive,
            normal,
            vec![Stroke::ctrl(Key::W), Stroke::key(Key::C)],
        ),
    ]);
    for scope in [Scope::VimInsert, Scope::VimReplace] {
        for binding in vscode_bindings()
            .into_iter()
            .filter(|binding| binding.rule.scope == Scope::DocumentEditor)
        {
            let mut binding = binding;
            binding.id = format!("vim.{scope:?}.{}", binding.id);
            binding.rule.scope = scope;
            rules.push(binding);
        }
    }
    rules
}

pub fn builtin_bindings(profile: &str) -> Option<Vec<BuiltinBinding>> {
    match profile {
        BUILTIN_VSCODE => Some(vscode_bindings()),
        BUILTIN_VIM => Some(vim_bindings()),
        _ => None,
    }
}

pub fn validate_catalog() -> Result<(), String> {
    let mut command_ids = HashSet::new();
    for info in CATALOG {
        if !command_ids.insert(info.id) {
            return Err(format!("duplicate command ID {}", info.id));
        }
        if info.scopes.is_empty() {
            return Err(format!("{} has no valid scopes", info.id));
        }
    }
    let mut binding_ids = HashSet::new();
    for profile in [BUILTIN_VSCODE, BUILTIN_VIM] {
        let bindings = builtin_bindings(profile).expect("known builtin");
        for binding in &bindings {
            if !binding_ids.insert(binding.id.clone()) {
                return Err(format!("duplicate built-in binding ID {}", binding.id));
            }
            validate_rule(&binding.rule)?;
        }
        for (index, binding) in bindings.iter().enumerate() {
            for other in bindings.iter().skip(index + 1) {
                if rules_conflict(&binding.rule, &other.rule) {
                    return Err(format!(
                        "built-in bindings {} and {} conflict",
                        binding.id, other.id
                    ));
                }
            }
        }
        validate_effective_sequences(
            &bindings
                .into_iter()
                .map(EffectiveBinding::builtin)
                .collect::<Vec<_>>(),
        )?;
    }
    Ok(())
}

pub fn validate_rule(rule: &BindingRule) -> Result<(), String> {
    if !(1..=4).contains(&rule.sequence.len()) {
        return Err("a key sequence must contain one to four strokes".into());
    }
    let command = Command::from_id(&rule.command)
        .ok_or_else(|| format!("unknown command {}", rule.command))?;
    if !command.info().scopes.contains(&rule.scope) {
        return Err(format!(
            "{} is not valid in {}",
            rule.command,
            rule.scope.label()
        ));
    }
    for stroke in &rule.sequence {
        let key = stroke
            .parsed_key()
            .ok_or_else(|| format!("unknown key {}", stroke.key))?;
        if stroke.primary && (stroke.ctrl || stroke.super_key) {
            return Err("Primary cannot be combined with Ctrl or Super".into());
        }
        if matches!(
            rule.scope,
            Scope::DocumentEditor
                | Scope::VimInsert
                | Scope::VimReplace
                | Scope::Find
                | Scope::ProjectSearch
                | Scope::Agent
                | Scope::Settings
        ) && printable(key)
            && !stroke.primary
            && !stroke.ctrl
            && !stroke.alt
            && !stroke.super_key
        {
            return Err("bare printable keys are text input in this scope".into());
        }
        if printable(key) && stroke.ctrl && stroke.alt && !stroke.physical {
            return Err("Ctrl+Alt printable bindings must use a physical key".into());
        }
    }
    Ok(())
}

pub fn validate_settings(settings: &KeybindingSettings) -> Result<(), String> {
    if settings.profiles.len() > MAX_CUSTOM_PROFILES {
        return Err(format!(
            "at most {MAX_CUSTOM_PROFILES} custom profiles are allowed"
        ));
    }
    if settings
        .profile_behavior(&settings.active_profile)
        .is_none()
    {
        return Err(format!(
            "active keybinding profile {} does not exist",
            settings.active_profile
        ));
    }
    let mut names = HashSet::new();
    let mut deviations = 0;
    for (id, profile) in &settings.profiles {
        if !id
            .strip_prefix("profile-")
            .is_some_and(|number| number.parse::<usize>().is_ok_and(|number| number > 0))
        {
            return Err(format!("invalid custom profile ID {id}"));
        }
        let name = profile.name.trim();
        if !(1..=64).contains(&name.chars().count()) {
            return Err(format!("{id} name must contain 1 to 64 characters"));
        }
        if !names.insert(name.to_lowercase()) {
            return Err(format!("duplicate profile name {}", profile.name));
        }
        if let Some(base) = profile.base.as_deref() {
            let behavior = builtin_behavior(base)
                .ok_or_else(|| format!("{id} has unknown built-in base {base}"))?;
            if behavior != profile.behavior {
                return Err(format!("{id} behavior does not match {base}"));
            }
            let known = builtin_bindings(base)
                .unwrap_or_default()
                .into_iter()
                .map(|binding| binding.id)
                .collect::<HashSet<_>>();
            if let Some(removed) = profile
                .removed
                .iter()
                .find(|removed| !known.contains(*removed))
            {
                return Err(format!("{id} removes unknown binding {removed}"));
            }
        } else if !profile.removed.is_empty() {
            return Err(format!("{id} has removed bindings without a base"));
        }
        let unique_removed = profile.removed.iter().collect::<HashSet<_>>();
        if unique_removed.len() != profile.removed.len() {
            return Err(format!("{id} removes the same binding more than once"));
        }
        deviations += profile.removed.len() + profile.bindings.len();
        validate_profile_rules(profile).map_err(|error| format!("{id}: {error}"))?;
    }
    if deviations > MAX_CUSTOM_DEVIATIONS {
        return Err(format!(
            "at most {MAX_CUSTOM_DEVIATIONS} custom bindings and removals are allowed"
        ));
    }
    for id in settings.profiles.keys() {
        validate_effective_sequences(&settings.effective_bindings_for(id)?)
            .map_err(|error| format!("{id}: {error}"))?;
    }
    Ok(())
}

fn validate_profile_rules(profile: &CustomProfile) -> Result<(), String> {
    for (index, rule) in profile.bindings.iter().enumerate() {
        validate_rule(rule)?;
        for other in profile.bindings.iter().skip(index + 1) {
            if rules_conflict(rule, other) {
                return Err(format!("{} conflicts with {}", rule.command, other.command));
            }
            if same_rule_domain(rule, other)
                && (sequence_prefix(&rule.sequence, &other.sequence)
                    || sequence_prefix(&other.sequence, &rule.sequence))
            {
                return Err("an exact binding cannot also be a chord prefix".into());
            }
        }
    }
    Ok(())
}

fn rules_conflict(left: &BindingRule, right: &BindingRule) -> bool {
    same_rule_domain(left, right) && left.sequence == right.sequence
}

fn same_rule_domain(left: &BindingRule, right: &BindingRule) -> bool {
    left.scope == right.scope && platforms_overlap(left.platform, right.platform)
}

fn platforms_overlap(left: Option<Platform>, right: Option<Platform>) -> bool {
    left.is_none() || right.is_none() || left == right
}

fn sequence_prefix(left: &[Stroke], right: &[Stroke]) -> bool {
    left.len() < right.len() && right.starts_with(left)
}

pub fn rules_have_sequence_conflict(left: &BindingRule, right: &BindingRule) -> bool {
    same_rule_domain(left, right)
        && (left.sequence == right.sequence
            || sequence_prefix(&left.sequence, &right.sequence)
            || sequence_prefix(&right.sequence, &left.sequence))
}

// ponytail: O(n²) is bounded to 256 deviations; add a prefix index only if profiling needs it.
fn validate_effective_sequences(bindings: &[EffectiveBinding]) -> Result<(), String> {
    for (index, binding) in bindings.iter().enumerate() {
        for other in bindings.iter().skip(index + 1) {
            if same_rule_domain(&binding.rule, &other.rule)
                && (sequence_prefix(&binding.rule.sequence, &other.rule.sequence)
                    || sequence_prefix(&other.rule.sequence, &binding.rule.sequence))
            {
                return Err(format!(
                    "{} and {} make an exact binding a chord prefix",
                    binding.rule.command, other.rule.command
                ));
            }
        }
    }
    Ok(())
}

fn printable(key: Key) -> bool {
    matches!(
        key,
        Key::Space
            | Key::Colon
            | Key::Comma
            | Key::Backslash
            | Key::Slash
            | Key::Pipe
            | Key::Questionmark
            | Key::Exclamationmark
            | Key::OpenBracket
            | Key::CloseBracket
            | Key::OpenCurlyBracket
            | Key::CloseCurlyBracket
            | Key::Backtick
            | Key::Minus
            | Key::Period
            | Key::Plus
            | Key::Equals
            | Key::Semicolon
            | Key::Quote
            | Key::Num0
            | Key::Num1
            | Key::Num2
            | Key::Num3
            | Key::Num4
            | Key::Num5
            | Key::Num6
            | Key::Num7
            | Key::Num8
            | Key::Num9
            | Key::A
            | Key::B
            | Key::C
            | Key::D
            | Key::E
            | Key::F
            | Key::G
            | Key::H
            | Key::I
            | Key::J
            | Key::K
            | Key::L
            | Key::M
            | Key::N
            | Key::O
            | Key::P
            | Key::Q
            | Key::R
            | Key::S
            | Key::T
            | Key::U
            | Key::V
            | Key::W
            | Key::X
            | Key::Y
            | Key::Z
    )
}

impl fmt::Display for Platform {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Macos => "macOS",
            Self::Windows => "Windows",
            Self::Linux => "Linux",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn catalog_ids_and_builtin_binding_ids_are_unique() {
        validate_catalog().unwrap();
    }

    #[test]
    fn custom_specific_rule_wins_over_global_and_builtin_rules() {
        let mut settings = KeybindingSettings::default();
        let id = settings
            .create_profile("Mine", Some(BUILTIN_VSCODE), Behavior::Standard)
            .unwrap();
        settings.profiles.get_mut(&id).unwrap().bindings.extend([
            BindingRule::new(
                Command::SearchFind,
                Scope::Global,
                vec![Stroke::primary(Key::E)],
            ),
            BindingRule::new(
                Command::EditorSelectAll,
                Scope::DocumentEditor,
                vec![Stroke::primary(Key::E)],
            ),
        ]);
        settings.active_profile = id;
        let mut resolver =
            Resolver::new(settings.effective_bindings().unwrap(), Platform::Macos).unwrap();

        let result = resolver.resolve(
            InputStroke::new(
                Key::E,
                None,
                Modifiers {
                    mac_cmd: true,
                    command: true,
                    ..Modifiers::NONE
                },
            ),
            &[Scope::DocumentEditor],
            false,
            Duration::ZERO,
        );

        assert_eq!(result.command, Some(Command::EditorSelectAll));
    }

    #[test]
    fn chord_reprocesses_an_invalid_second_stroke() {
        let settings = KeybindingSettings::default();
        let mut resolver =
            Resolver::new(settings.effective_bindings().unwrap(), Platform::Macos).unwrap();
        let modifiers = Modifiers {
            mac_cmd: true,
            command: true,
            ..Modifiers::NONE
        };
        let first = resolver.resolve(
            InputStroke::new(Key::K, None, modifiers),
            &[Scope::Global],
            false,
            Duration::ZERO,
        );
        assert!(first.consumed && first.command.is_none());

        let second = resolver.resolve(
            InputStroke::new(Key::F, None, modifiers),
            &[Scope::Global],
            false,
            Duration::from_millis(20),
        );

        assert_eq!(second.command, Some(Command::SearchFind));
    }

    #[test]
    fn logical_primary_uses_egui_command_modifier() {
        let settings = KeybindingSettings::default();
        let mut resolver =
            Resolver::new(settings.effective_bindings().unwrap(), Platform::Macos).unwrap();

        let result = resolver.resolve(
            InputStroke::new(
                Key::S,
                Some(Key::S),
                Modifiers {
                    command: true,
                    ..Modifiers::NONE
                },
            ),
            &[Scope::DocumentEditor],
            false,
            Duration::ZERO,
        );

        assert_eq!(result.command, Some(Command::FileSave));

        let result = resolver.resolve(
            InputStroke::new(
                Key::F,
                Some(Key::F),
                Modifiers {
                    command: true,
                    shift: true,
                    ..Modifiers::NONE
                },
            ),
            &[Scope::Find],
            false,
            Duration::ZERO,
        );
        assert_eq!(result.command, Some(Command::SearchProject));
    }

    #[test]
    fn interface_scale_shortcuts_are_global_and_customizable() {
        let mut resolver = Resolver::new(
            KeybindingSettings::default().effective_bindings().unwrap(),
            Platform::Macos,
        )
        .unwrap();
        let modifiers = Modifiers {
            command: true,
            shift: true,
            ..Modifiers::NONE
        };

        let increase = resolver.resolve(
            InputStroke::new(Key::Plus, Some(Key::Equals), modifiers),
            &[Scope::Settings],
            false,
            Duration::ZERO,
        );
        let decrease = resolver.resolve(
            InputStroke::new(Key::Minus, Some(Key::Minus), modifiers),
            &[Scope::Terminal],
            false,
            Duration::ZERO,
        );

        assert_eq!(increase.command, Some(Command::AppIncreaseUiScale));
        assert_eq!(decrease.command, Some(Command::AppDecreaseUiScale));
    }

    #[test]
    fn vscode_platform_defaults_resolve_the_documented_differences() {
        let primary = |platform: Platform, shift: bool| Modifiers {
            ctrl: platform != Platform::Macos,
            mac_cmd: platform == Platform::Macos,
            command: true,
            shift,
            ..Modifiers::NONE
        };
        for (platform, close_key, close_modifiers, redo_key, redo_modifiers) in [
            (
                Platform::Macos,
                Key::W,
                primary(Platform::Macos, false),
                Key::Z,
                primary(Platform::Macos, true),
            ),
            (
                Platform::Windows,
                Key::F4,
                Modifiers {
                    ctrl: true,
                    command: true,
                    ..Modifiers::NONE
                },
                Key::Y,
                primary(Platform::Windows, false),
            ),
            (
                Platform::Linux,
                Key::W,
                primary(Platform::Linux, false),
                Key::Y,
                primary(Platform::Linux, false),
            ),
        ] {
            let mut resolver = Resolver::new(vscode_effective(), platform).unwrap();
            assert_eq!(
                resolver
                    .resolve(
                        InputStroke::new(Key::S, Some(Key::S), primary(platform, false)),
                        &[Scope::DocumentEditor],
                        false,
                        Duration::ZERO,
                    )
                    .command,
                Some(Command::FileSave)
            );
            assert_eq!(
                resolver
                    .resolve(
                        InputStroke::new(close_key, Some(close_key), close_modifiers),
                        &[Scope::DocumentEditor],
                        false,
                        Duration::ZERO,
                    )
                    .command,
                Some(Command::FileCloseActive)
            );
            assert_eq!(
                resolver
                    .resolve(
                        InputStroke::new(redo_key, Some(redo_key), redo_modifiers),
                        &[Scope::DocumentEditor],
                        false,
                        Duration::ZERO,
                    )
                    .command,
                Some(Command::EditorRedo)
            );
            assert!(
                resolver
                    .resolve(
                        InputStroke::new(Key::K, Some(Key::K), primary(platform, false)),
                        &[Scope::DocumentEditor],
                        false,
                        Duration::ZERO,
                    )
                    .consumed
            );
            assert_eq!(
                resolver
                    .resolve(
                        InputStroke::new(Key::S, Some(Key::S), primary(platform, false)),
                        &[Scope::DocumentEditor],
                        false,
                        Duration::from_millis(10),
                    )
                    .command,
                Some(Command::AppOpenKeybindings)
            );
        }
    }

    fn vscode_effective() -> Vec<EffectiveBinding> {
        vscode_bindings()
            .into_iter()
            .map(EffectiveBinding::builtin)
            .collect()
    }

    #[test]
    fn vim_operator_pending_reuses_operator_and_motion_keys() {
        let mut resolver = Resolver::new(
            builtin_bindings(BUILTIN_VIM)
                .unwrap()
                .into_iter()
                .map(|binding| EffectiveBinding {
                    id: binding.id,
                    rule: binding.rule,
                    source: BindingSource::BuiltIn,
                })
                .collect(),
            Platform::Macos,
        )
        .unwrap();

        for (key, expected) in [
            (Key::D, Command::VimOperatorDelete),
            (Key::W, Command::VimWordForward),
        ] {
            let result = resolver.resolve(
                InputStroke::new(key, Some(key), Modifiers::NONE),
                &[Scope::VimOperator],
                false,
                Duration::ZERO,
            );
            assert_eq!(result.command, Some(expected));
        }
    }

    #[test]
    fn profile_edits_reject_prefixes_without_leaving_partial_state() {
        let mut settings = KeybindingSettings::default();
        let id = settings
            .create_profile("Mine", Some(BUILTIN_VSCODE), Behavior::Standard)
            .unwrap();
        settings
            .add_binding(
                &id,
                BindingRule::new(
                    Command::AppOpenSettings,
                    Scope::Global,
                    vec![Stroke::primary(Key::K), Stroke::primary(Key::A)],
                ),
                false,
            )
            .unwrap();
        let before = settings.profiles[&id].bindings.clone();

        assert!(
            settings
                .add_binding(
                    &id,
                    BindingRule::new(
                        Command::AppOpenSettings,
                        Scope::Global,
                        vec![Stroke::primary(Key::K)],
                    ),
                    false,
                )
                .is_err()
        );
        assert_eq!(settings.profiles[&id].bindings, before);

        settings.profiles.get_mut(&id).unwrap().bindings = vec![BindingRule::new(
            Command::AppOpenSettings,
            Scope::Global,
            vec![Stroke::primary(Key::K)],
        )];
        assert!(validate_settings(&settings).is_err());
    }

    #[test]
    fn logical_physical_altgr_timeout_escape_and_repeat_are_deterministic() {
        let logical = BindingRule::new(
            Command::VimMoveLeft,
            Scope::VimNormal,
            vec![Stroke::key(Key::A)],
        );
        let mut physical_stroke = Stroke::key(Key::A);
        physical_stroke.ctrl = true;
        physical_stroke.alt = true;
        physical_stroke.physical = true;
        let physical = BindingRule::new(
            Command::VimMoveRight,
            Scope::VimNormal,
            vec![physical_stroke],
        );
        let mut resolver = Resolver::new(
            [logical, physical]
                .into_iter()
                .enumerate()
                .map(|(index, rule)| EffectiveBinding {
                    id: index.to_string(),
                    source: BindingSource::Custom,
                    rule,
                })
                .collect(),
            Platform::Windows,
        )
        .unwrap();
        assert_eq!(
            resolver
                .resolve(
                    InputStroke::new(Key::Z, Some(Key::A), Modifiers::NONE),
                    &[Scope::VimNormal],
                    false,
                    Duration::ZERO,
                )
                .command,
            None
        );
        assert_eq!(
            resolver
                .resolve(
                    InputStroke::new(
                        Key::Z,
                        Some(Key::A),
                        Modifiers {
                            ctrl: true,
                            alt: true,
                            ..Modifiers::NONE
                        },
                    ),
                    &[Scope::VimNormal],
                    false,
                    Duration::ZERO,
                )
                .command,
            Some(Command::VimMoveRight)
        );

        let mut resolver = Resolver::new(
            KeybindingSettings::default().effective_bindings().unwrap(),
            Platform::Macos,
        )
        .unwrap();
        let command = Modifiers {
            command: true,
            ..Modifiers::NONE
        };
        assert!(
            resolver
                .resolve(
                    InputStroke::new(Key::K, Some(Key::K), command),
                    &[Scope::Global],
                    false,
                    Duration::ZERO,
                )
                .consumed
        );
        assert!(
            resolver
                .resolve(
                    InputStroke::new(Key::Escape, Some(Key::Escape), Modifiers::NONE),
                    &[Scope::Global],
                    false,
                    Duration::from_millis(10),
                )
                .consumed
        );
        assert!(resolver.pending_label().is_none());
        assert_eq!(
            resolver
                .resolve(
                    InputStroke::new(Key::S, Some(Key::S), command),
                    &[Scope::Global],
                    true,
                    Duration::from_secs(2),
                )
                .command,
            None
        );
    }

    #[test]
    fn standard_editor_rejects_every_bare_printable_binding() {
        let mut physical = Stroke::key(Key::A);
        physical.physical = true;
        assert!(
            validate_rule(&BindingRule::new(
                Command::EditorCursorLeft,
                Scope::DocumentEditor,
                vec![physical],
            ))
            .is_err()
        );
        assert!(
            validate_rule(&BindingRule::new(
                Command::SearchNext,
                Scope::Find,
                vec![Stroke::key(Key::N)],
            ))
            .is_err()
        );
        assert!(
            validate_rule(&BindingRule::new(
                Command::VimNormal,
                Scope::VimInsert,
                vec![Stroke::key(Key::I)],
            ))
            .is_err()
        );
    }

    #[test]
    fn terminal_controls_fall_through_builtins_but_explicit_custom_globals_win() {
        let rule = BindingRule::new(
            Command::AppOpenSettings,
            Scope::Global,
            vec![Stroke::ctrl(Key::C)],
        );
        let input = InputStroke::new(
            Key::C,
            Some(Key::C),
            Modifiers {
                ctrl: true,
                command: true,
                ..Modifiers::NONE
            },
        );
        let mut builtin = Resolver::new(
            vec![EffectiveBinding {
                id: "builtin".into(),
                source: BindingSource::BuiltIn,
                rule: rule.clone(),
            }],
            Platform::Linux,
        )
        .unwrap();
        assert!(
            !builtin
                .resolve(input, &[Scope::Terminal], false, Duration::ZERO)
                .consumed
        );

        let mut custom = Resolver::new(
            vec![EffectiveBinding {
                id: "custom".into(),
                source: BindingSource::Custom,
                rule,
            }],
            Platform::Linux,
        )
        .unwrap();
        assert_eq!(
            custom
                .resolve(input, &[Scope::Terminal], false, Duration::ZERO)
                .command,
            Some(Command::AppOpenSettings)
        );
    }

    #[test]
    fn profile_crud_keeps_only_bounded_builtin_deviations() {
        let mut settings = KeybindingSettings::default();
        let derived = settings.derive_profile(BUILTIN_VSCODE).unwrap();
        settings.rename_profile(&derived, "Work").unwrap();
        settings
            .disable_binding(&derived, "vscode.view.sidebar")
            .unwrap();
        settings
            .add_binding(
                &derived,
                BindingRule::new(
                    Command::ViewToggleSidebar,
                    Scope::Global,
                    vec![Stroke::primary(Key::E)],
                ),
                false,
            )
            .unwrap();
        let copy = settings.duplicate_profile(&derived).unwrap();
        assert_eq!(
            settings.profiles[&copy].base.as_deref(),
            Some(BUILTIN_VSCODE)
        );
        assert_eq!(settings.profiles[&copy].bindings.len(), 1);
        settings
            .reset_command(&copy, Command::ViewToggleSidebar)
            .unwrap();
        assert!(settings.profiles[&copy].bindings.is_empty());
        assert!(settings.profiles[&copy].removed.is_empty());
        settings.delete_profile(&copy, BUILTIN_VIM).unwrap();
        assert_eq!(settings.active_profile, BUILTIN_VIM);

        while settings.profiles.len() < MAX_CUSTOM_PROFILES {
            let number = settings.profiles.len();
            settings
                .create_profile(&format!("Profile {number}"), None, Behavior::Standard)
                .unwrap();
        }
        assert!(
            settings
                .create_profile("One too many", None, Behavior::Standard)
                .is_err()
        );
        validate_settings(&settings).unwrap();
    }
}
