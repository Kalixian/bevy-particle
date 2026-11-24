//! Adding, removing and ordering scenes and emitters, and emitter files.

use bevy::prelude::*;
use bevy_egui::egui::{self, Ui};
use bevy_particle::{EffectFile, EmitterSource};

use super::{
    documents::{self, document_path, Documents},
    handles,
    inspector::Selection,
    platform, theme,
};
use crate::demos::{effect_path, DemoList};

/// A question to the user, shown in the panel.
pub struct Dialog {
    action: Action,
    /// What's typed in.
    name: String,
    error: Option<String>,
    /// The text field hasn't been given the keyboard yet.
    new: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Action {
    NewScene,
    DuplicateScene,
    RemoveScene,
    /// Moves this emitter of the scene to a file of its own.
    EmitterFile(usize),
}

impl Dialog {
    fn new(action: Action, name: String) -> Self {
        Self { action, name, error: None, new: true }
    }

    /// For moving an inline emitter of a scene to a file.
    pub fn emitter_file(scene: &str, index: usize, documents: &Documents) -> Self {
        let base = format!("{}-emitter", scene_name(scene));
        let name = documents::free_name(&base, |name| emitter_taken(name, documents));
        Self::new(Action::EmitterFile(index), name)
    }
}

fn scene_name(scene: &str) -> &str {
    let file = scene.rsplit('/').next().unwrap_or(scene);
    file.strip_suffix(".effect.ron").unwrap_or(file)
}

fn emitter_file_path(name: &str) -> String {
    format!("effects/{name}.emitter.ron")
}

fn scene_taken(name: &str, documents: &Documents) -> bool {
    let path = effect_path(name);
    documents.is_open(&path) || platform::exists(&path)
}

fn emitter_taken(name: &str, documents: &Documents) -> bool {
    let path = emitter_file_path(name);
    documents.is_open(&path) || platform::exists(&path)
}

/// How a scene refers to the emitter file at `path`.
fn reference(scene: &str, path: &str) -> String {
    let folder = scene.rfind('/').map_or("", |end| &scene[..=end]);
    match path.strip_prefix(folder) {
        Some(file) if !file.contains('/') => file.to_string(),
        _ => format!("/{path}"),
    }
}

fn swap_scenes(list: &DemoList, a: usize, b: usize) -> DemoList {
    let mut list = list.clone();
    list.scenes.swap(a, b);
    list
}

/// The menu of things to do with scenes.
pub fn scene_menu(
    ui: &mut Ui,
    scene: &str,
    documents: &mut Documents,
    dialog: &mut Option<Dialog>,
) {
    let Some(list) = documents.list().cloned() else {
        return;
    };
    let name = scene_name(scene);
    let index = list.scenes.iter().position(|other| other == name);
    let mut changed = None;
    ui.menu_button("Scene", |ui| {
        if ui.button("New…").clicked() {
            let name = documents::free_name("scene", |name| scene_taken(name, documents));
            *dialog = Some(Dialog::new(Action::NewScene, name));
        }
        let open = documents.scene(scene).is_some();
        if ui.add_enabled(open, egui::Button::new("Duplicate…")).clicked() {
            let base = format!("{name}-copy");
            let name = documents::free_name(&base, |name| scene_taken(name, documents));
            *dialog = Some(Dialog::new(Action::DuplicateScene, name));
        }
        ui.separator();
        let Some(index) = index else {
            return;
        };
        let button = ui.add_enabled(index > 0, egui::Button::new("Move up the list"));
        if button.clicked() {
            changed = Some(swap_scenes(&list, index, index - 1));
        }
        let can_move = index + 1 < list.scenes.len();
        let button = ui.add_enabled(can_move, egui::Button::new("Move down the list"));
        if button.clicked() {
            changed = Some(swap_scenes(&list, index, index + 1));
        }
        let button = egui::Button::new("Show first at startup").selected(list.start == name);
        if ui.add(button).clicked() {
            changed = Some(DemoList { start: name.into(), ..list.clone() });
        }
        ui.separator();
        handles::toggle_button(ui);
        ui.separator();
        if ui.add_enabled(list.scenes.len() > 1, egui::Button::new("Remove…")).clicked() {
            *dialog = Some(Dialog::new(Action::RemoveScene, String::new()));
        }
    });
    if let Some(list) = changed {
        documents.edit_list(list);
        documents.end_group();
    }
}

/// Shows the dialog, if there is one. Returns the name of a scene to show.
pub fn show_dialog(
    ui: &mut Ui,
    scene: &str,
    documents: &mut Documents,
    current: &mut Option<Dialog>,
) -> Option<String> {
    let dialog = current.as_mut()?;
    let mut done = false;
    let mut shown = None;
    let fill = egui::Color32::from_gray(38);
    egui::Frame::new().fill(fill).corner_radius(4).inner_margin(8).show(ui, |ui| {
        let name = scene_name(scene);
        let (question, confirm) = match dialog.action {
            Action::NewScene => ("Name of the new scene".into(), "Create"),
            Action::DuplicateScene => (format!("Name of the copy of {name}"), "Duplicate"),
            Action::RemoveScene => {
                (format!("Take {name} off the list? Its file stays."), "Remove")
            }
            Action::EmitterFile(_) => ("Name of the emitter file".into(), "Create"),
        };
        ui.label(question);
        let mut entered = false;
        if dialog.action != Action::RemoveScene {
            let field = ui.add(
                egui::TextEdit::singleline(&mut dialog.name).desired_width(f32::INFINITY),
            );
            if std::mem::take(&mut dialog.new) {
                field.request_focus();
            }
            entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        }
        if let Some(error) = &dialog.error {
            ui.colored_label(theme::ERROR, error);
        }
        ui.horizontal(|ui| {
            if ui.button(confirm).clicked() || entered {
                match confirm_dialog(scene, dialog, documents) {
                    Ok(name) => {
                        shown = name;
                        done = true;
                    }
                    Err(error) => dialog.error = Some(error.to_string()),
                }
            }
            done |= ui.button("Cancel").clicked();
        });
    });
    if done {
        *current = None;
    }
    shown
}

fn confirm_dialog(
    scene: &str,
    dialog: &Dialog,
    documents: &mut Documents,
) -> Result<Option<String>, BevyError> {
    let name = dialog.name.trim();
    let current = scene_name(scene);
    match dialog.action {
        Action::RemoveScene => {
            documents.remove_scene(current);
            Ok(None)
        }
        Action::NewScene | Action::DuplicateScene => {
            documents::check_name(name)?;
            if scene_taken(name, documents) {
                return Err(format!("there's a scene called {name} already").into());
            }
            let file = match dialog.action {
                Action::DuplicateScene => documents.scene(scene).ok_or("it isn't open")?.clone(),
                _ => documents::new_scene(),
            };
            documents.add_scene(name, file, Some(current))?;
            Ok(Some(name.into()))
        }
        Action::EmitterFile(index) => {
            documents::check_name(name)?;
            if emitter_taken(name, documents) {
                return Err(format!("there's an emitter file called {name} already").into());
            }
            let mut file = documents.scene(scene).ok_or("the scene isn't open")?.clone();
            let entry = file.emitters.get_mut(index).ok_or("the emitter is gone")?;
            let EmitterSource::Inline(def) = &entry.source else {
                return Err("it's in a file already".into());
            };
            let path = emitter_file_path(name);
            documents.add_emitter(&path, (**def).clone())?;
            entry.source = EmitterSource::File(reference(scene, &path));
            documents.edit_scene(scene, file);
            documents.end_group();
            Ok(None)
        }
    }
}

/// Buttons that add emitters to a scene, or change the selected one's place.
pub fn emitter_buttons(
    ui: &mut Ui,
    scene: &str,
    file: &mut EffectFile,
    selection: &mut Selection,
    documents: &Documents,
) {
    let selected = match *selection {
        Selection::Emitter(index) if index < file.emitters.len() => Some(index),
        _ => None,
    };
    let count = file.emitters.len();
    ui.horizontal(|ui| {
        ui.menu_button("Add emitter", |ui| {
            let mut source = None;
            if ui.button("New").clicked() {
                source = Some(EmitterSource::Inline(default()));
            }
            ui.separator();
            ui.weak("From a file");
            for path in documents.emitter_files() {
                let name = path.rsplit('/').next().unwrap_or(path);
                if ui.button(name).clicked() {
                    source = Some(EmitterSource::File(reference(scene, path)));
                }
            }
            if let Some(source) = source {
                let mut emitter = documents::new_emitter();
                emitter.source = source;
                file.emitters.push(emitter);
                *selection = Selection::Emitter(count);
            }
        });
        let button = |ui: &mut Ui, text: &str, hint: &str, enabled: bool| {
            ui.add_enabled(enabled, egui::Button::new(text)).on_hover_text(hint).clicked()
        };
        if button(ui, "Duplicate", "", selected.is_some())
            && let Some(index) = selected
        {
            file.emitters.insert(index + 1, file.emitters[index].clone());
            *selection = Selection::Emitter(index + 1);
        }
        if button(ui, "⬆", "Move up", selected.is_some_and(|index| index > 0))
            && let Some(index) = selected
        {
            file.emitters.swap(index, index - 1);
            *selection = Selection::Emitter(index - 1);
        }
        if button(ui, "⬇", "Move down", selected.is_some_and(|index| index + 1 < count))
            && let Some(index) = selected
        {
            file.emitters.swap(index, index + 1);
            *selection = Selection::Emitter(index + 1);
        }
        if button(ui, "✖", "Remove", selected.is_some())
            && let Some(index) = selected
        {
            file.emitters.remove(index);
            *selection = match count - 1 {
                0 => Selection::Scene,
                left => Selection::Emitter(index.min(left - 1)),
            };
        }
    });
}

/// Where an emitter is kept: in its scene, or in a file that scenes can share. Returns whether to
/// ask for the name of a file to move it to.
pub fn show_source(
    ui: &mut Ui,
    scene: &str,
    source: &mut EmitterSource,
    documents: &Documents,
) -> bool {
    let mut ask = false;
    ui.horizontal_wrapped(|ui| match source {
        EmitterSource::Inline(_) => {
            ui.weak("Kept in this scene");
            let hint = "So that other scenes can use it too";
            ask = ui.small_button("Move to a file…").on_hover_text(hint).clicked();
        }
        EmitterSource::File(file) => {
            ui.weak(format!("Kept in {}", file.rsplit('/').next().unwrap_or(file)));
            let path = bevy::asset::AssetPath::parse(scene).resolve_embed_str(file);
            let def = path.ok().and_then(|path| documents.emitter(&document_path(&path)));
            let hint = "Copies it into this scene, so that changes to it stay here";
            let button = ui.add_enabled(def.is_some(), egui::Button::new("Make unique").small());
            if button.on_hover_text(hint).clicked()
                && let Some(def) = def
            {
                *source = EmitterSource::Inline(Box::new(def.clone()));
            }
        }
    });
    ask
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCENE: &str = "effects/a.effect.ron";

    fn open() -> Documents {
        let mut documents = Documents::default();
        let scene = "(space: (capacity: 8), emitters: [(source: Inline((emitter: (rate: 3.0))))])";
        documents.file_loaded(SCENE, scene.into()).unwrap();
        let list = "(start: \"a\", scenes: [\"a\"])";
        documents.file_loaded(crate::demos::LIST_PATH, list.into()).unwrap();
        documents
    }

    #[test]
    fn emitter_files_are_referred_to_from_the_scenes_folder() {
        assert_eq!(reference(SCENE, "effects/e.emitter.ron"), "e.emitter.ron");
        assert_eq!(reference(SCENE, "effects/x/e.emitter.ron"), "/effects/x/e.emitter.ron");
        assert_eq!(reference(SCENE, "other/e.emitter.ron"), "/other/e.emitter.ron");
        let resolve = |reference: &str| {
            document_path(&bevy::asset::AssetPath::parse(SCENE).resolve_embed_str(reference).unwrap())
        };
        for path in ["effects/e.emitter.ron", "other/e.emitter.ron"] {
            assert_eq!(resolve(&reference(SCENE, path)), path);
        }
    }

    #[test]
    fn emitters_move_to_files() {
        let mut documents = open();
        // The name must not be that of a shipped file.
        let dialog = Dialog::new(Action::EmitterFile(0), "test-only-emitter".into());
        confirm_dialog(SCENE, &dialog, &mut documents).unwrap();
        let path = "effects/test-only-emitter.emitter.ron";
        assert_eq!(documents.files_of(SCENE), [SCENE, path]);
        assert_eq!(documents.emitter(path).unwrap().emitter.rate, 3.0);
        let effect = documents.build(SCENE, |_| default()).unwrap();
        assert_eq!(effect.emitters[0].def.emitter.rate, 3.0);
        documents.undo(SCENE);
        assert!(documents.dirty_files(None).is_empty());
    }

    #[test]
    fn names_are_checked() {
        let mut documents = open();
        for (action, name) in [
            (Action::NewScene, "a"),
            (Action::NewScene, "../b"),
            (Action::NewScene, ""),
            (Action::DuplicateScene, "A"),
            (Action::EmitterFile(0), "x/y"),
            (Action::EmitterFile(7), "test-only-emitter"),
        ] {
            let dialog = Dialog::new(action, name.into());
            assert!(confirm_dialog(SCENE, &dialog, &mut documents).is_err(), "{name}");
        }
        assert!(documents.dirty_files(None).is_empty());
    }

    #[test]
    fn duplicates_follow_their_scene() {
        let mut documents = open();
        let dialog = Dialog::new(Action::DuplicateScene, "test-only-scene".into());
        let shown = confirm_dialog(SCENE, &dialog, &mut documents).unwrap();
        assert_eq!(shown.as_deref(), Some("test-only-scene"));
        let copy = documents.scene(&effect_path("test-only-scene"));
        assert_eq!(copy, documents.scene(SCENE));
        assert_eq!(documents.list().unwrap().scenes, ["a", "test-only-scene"]);
    }
}
