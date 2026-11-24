//! Getting files out of the app and back in: on the desktop, the asset folder; on the web,
//! downloads and uploads.

#[cfg(not(target_arch = "wasm32"))]
pub use desktop::*;
#[cfg(target_arch = "wasm32")]
pub use web::*;

#[cfg(not(target_arch = "wasm32"))]
mod desktop {
    use std::{
        fs, io,
        path::{Component, Path, PathBuf},
    };

    use bevy::{asset::io::file::FileAssetReader, prelude::*};

    /// Where an asset is. Fails for paths that lead out of the asset folder.
    fn file_path(path: &str) -> Result<PathBuf, BevyError> {
        let path = Path::new(path);
        if !path.components().all(|part| matches!(part, Component::Normal(_))) {
            return Err(format!("{} isn't in the asset folder", path.display()).into());
        }
        // The default `AssetPlugin::file_path`.
        Ok(FileAssetReader::get_base_path().join("assets").join(path))
    }

    /// The text of an asset as it is now; `None` if there's no such file.
    pub fn read(path: &str) -> Result<Option<String>, BevyError> {
        match fs::read_to_string(file_path(path)?) {
            Ok(text) => Ok(Some(text)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Whether there's an asset file that a new one would replace.
    pub fn exists(path: &str) -> bool {
        file_path(path).is_ok_and(|path| path.exists())
    }

    /// Replaces an asset file.
    pub fn save(path: &str, text: &str) -> Result<(), BevyError> {
        Ok(replace_file(&file_path(path)?, text)?)
    }

    /// The images in the texture folder, as paths in the asset folder.
    pub fn list_textures() -> Vec<String> {
        let files = file_path("textures").ok().and_then(|folder| fs::read_dir(folder).ok());
        files
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .filter(|name| name.ends_with(".png"))
            .map(|name| format!("textures/{name}"))
            .collect()
    }

    /// Never leaves a half-written file behind.
    pub(super) fn replace_file(path: &Path, text: &str) -> io::Result<()> {
        let mut temporary = path.as_os_str().to_owned();
        temporary.push(".tmp");
        let result = fs::write(&temporary, text).and_then(|()| fs::rename(&temporary, path));
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn paths_stay_in_the_asset_folder() {
            assert!(file_path("effects/a.effect.ron").is_ok());
            for path in ["../a.ron", "effects/../../a.ron", "/a.ron", "C:/a.ron"] {
                // Drive letters only mean something on Windows.
                if cfg!(windows) || !path.contains(':') {
                    assert!(file_path(path).is_err(), "{path}");
                }
            }
        }

        #[test]
        fn saving_replaces_the_file_and_leaves_nothing_else() {
            let folder = std::env::temp_dir().join(format!("bevy_particle_save_{}", std::process::id()));
            fs::create_dir_all(&folder).unwrap();
            let path = folder.join("a.effect.ron");
            fs::write(&path, "old and longer").unwrap();
            replace_file(&path, "new").unwrap();
            assert_eq!(fs::read_to_string(&path).unwrap(), "new");
            assert_eq!(fs::read_dir(&folder).unwrap().count(), 1);
            fs::remove_dir_all(folder).unwrap();
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    };

    use bevy::prelude::*;
    use wasm_bindgen::{closure::Closure, JsCast, JsValue};
    use web_sys::{
        BeforeUnloadEvent, Blob, HtmlAnchorElement, HtmlElement, HtmlInputElement, Url,
    };

    /// Files the user picked: (name, text or error).
    static UPLOADS: Mutex<Vec<(String, Result<String, String>)>> = Mutex::new(Vec::new());
    static UNSAVED: AtomicBool = AtomicBool::new(false);

    fn error(value: JsValue) -> BevyError {
        format!("{value:?}").into()
    }

    /// Adds a hidden element to the page. Some browsers ignore clicks on elements outside it.
    fn add_element<T: JsCast>(tag: &str) -> Result<T, BevyError> {
        let document = web_sys::window().and_then(|window| window.document()).ok_or("no document")?;
        let element = document.create_element(tag).map_err(error)?;
        let element: HtmlElement = element.dyn_into().map_err(|_| "not an HTML element")?;
        element.set_hidden(true);
        document.body().ok_or("no body")?.append_child(&element).map_err(error)?;
        Ok(element.dyn_into().map_err(|_| format!("not a <{tag}>"))?)
    }

    /// Downloads don't replace anything.
    pub fn exists(_path: &str) -> bool {
        false
    }

    /// Downloads an asset.
    pub fn save(path: &str, text: &str) -> Result<(), BevyError> {
        let parts = js_sys::Array::of1(&JsValue::from_str(text));
        let blob = Blob::new_with_str_sequence(&parts).map_err(error)?;
        let url = Url::create_object_url_with_blob(&blob).map_err(error)?;
        let anchor: HtmlAnchorElement = add_element("a")?;
        anchor.set_href(&url);
        anchor.set_download(path.rsplit('/').next().unwrap_or(path));
        anchor.click();
        anchor.remove();
        // The browser needs the URL until the download has started.
        let revoke = Closure::once_into_js(move || {
            let _ = Url::revoke_object_url(&url);
        });
        web_sys::window()
            .ok_or("no window")?
            .set_timeout_with_callback_and_timeout_and_arguments_0(
                revoke.unchecked_ref(),
                60_000,
            )
            .map_err(error)?;
        Ok(())
    }

    /// Folders can't be listed here.
    pub fn list_textures() -> Vec<String> {
        Vec::new()
    }

    /// Lets the user pick files; see [`take_uploads`].
    pub fn pick_files() -> Result<(), BevyError> {
        let input: HtmlInputElement = add_element("input")?;
        input.set_type("file");
        input.set_accept(".ron");
        input.set_multiple(true);
        let picker = input.clone();
        let on_change = Closure::once_into_js(move || {
            picker.remove();
            let Some(files) = picker.files() else {
                return;
            };
            let files: Vec<_> = (0..files.length()).filter_map(|i| files.get(i)).collect();
            // All at once, since they may need each other.
            wasm_bindgen_futures::spawn_local(async move {
                let mut uploads = Vec::new();
                for file in files {
                    let text = wasm_bindgen_futures::JsFuture::from(file.text()).await;
                    let text = text
                        .map(|text| text.as_string().unwrap_or_default())
                        .map_err(|error| format!("{error:?}"));
                    uploads.push((file.name(), text));
                }
                UPLOADS.lock().unwrap().extend(uploads);
            });
        });
        input.set_onchange(Some(on_change.unchecked_ref()));
        let canceled = input.clone();
        let on_cancel = Closure::once_into_js(move || canceled.remove());
        input.add_event_listener_with_callback("cancel", on_cancel.unchecked_ref()).map_err(error)?;
        input.click();
        Ok(())
    }

    /// The files picked since the last call.
    pub fn take_uploads() -> Vec<(String, Result<String, String>)> {
        std::mem::take(&mut UPLOADS.lock().unwrap())
    }

    /// Has the browser ask before leaving the page while there are unsaved changes.
    pub fn set_unsaved(unsaved: bool) {
        UNSAVED.store(unsaved, Ordering::Relaxed);
        static INSTALLED: AtomicBool = AtomicBool::new(false);
        if unsaved && !INSTALLED.swap(true, Ordering::Relaxed) {
            let ask = Closure::<dyn FnMut(BeforeUnloadEvent)>::new(|event: BeforeUnloadEvent| {
                if UNSAVED.load(Ordering::Relaxed) {
                    event.prevent_default();
                    event.set_return_value("unsaved changes");
                }
            });
            if let Some(window) = web_sys::window() {
                let _ = window
                    .add_event_listener_with_callback("beforeunload", ask.as_ref().unchecked_ref());
            }
            ask.forget(); // Lives as long as the page.
        }
    }
}
