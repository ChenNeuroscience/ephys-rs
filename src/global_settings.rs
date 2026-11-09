//! Global settings: settings for the whole program on this machine (where a vendor's library is, say), rather than for
//! a graph. Each type is a tab of its own in Settings…, kept between sessions, and its value now can be had from any
//! thread (`current`).
//!
//! A node names the global settings it reads (`Node::GlobalSettings`), and reads them through that
//! (`Node::global_settings`); a build has a tab for each type its nodes name (`NodesEnum::global_settings_types`). So
//! a component's global settings are in a build whenever a node that needs them is, without the build listing them.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use arc_swap::ArcSwap;
use eframe::egui;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::theme::Colours;

/// A type of global settings.
pub trait GlobalSettings: Default + Clone + PartialEq + Serialize + DeserializeOwned + Send + Sync + 'static {
    /// Its tab's name in Settings…, and what it's kept by between sessions: renaming it loses what was kept.
    const NAME: &'static str;

    /// Its tab, editing it. Changes apply as they're made.
    fn ui(&mut self, ui: &mut egui::Ui, colours: &Colours);
}

type Values = HashMap<TypeId, Arc<dyn Any + Send + Sync>>;

/// Each type's value now, by type: read far more than it's changed.
static VALUES: LazyLock<ArcSwap<Values>> = LazyLock::new(|| ArcSwap::from_pointee(HashMap::new()));

/// `T`'s value now (its default, if it's never been set).
pub fn current<T: GlobalSettings>() -> Arc<T> {
    VALUES.load().get(&TypeId::of::<T>()).and_then(|v| v.clone().downcast().ok()).unwrap_or_default()
}

/// Sets `T`'s value, from now on.
pub fn set<T: GlobalSettings>(value: T) {
    let value: Arc<dyn Any + Send + Sync> = Arc::new(value);
    VALUES.rcu(|values| {
        let mut values = Values::clone(values);
        values.insert(TypeId::of::<T>(), value.clone());
        values
    });
}

/// The global settings a node reads: `()` for none, a type of them, or a pair or triple of types.
pub trait GlobalSettingsSet: 'static {
    /// Their values now: an `Arc` of each.
    type Current;

    fn current() -> Self::Current;

    /// Adds each type to `types`, if it isn't there.
    fn types(types: &mut Vec<GlobalSettingsType>);
}

impl GlobalSettingsSet for () {
    type Current = ();

    fn current() {}

    fn types(_types: &mut Vec<GlobalSettingsType>) {}
}

impl<T: GlobalSettings> GlobalSettingsSet for T {
    type Current = Arc<T>;

    fn current() -> Arc<T> {
        current::<T>()
    }

    fn types(types: &mut Vec<GlobalSettingsType>) {
        if !types.iter().any(|t| t.type_id == TypeId::of::<T>()) {
            types.push(GlobalSettingsType::of::<T>());
        }
    }
}

impl<A: GlobalSettings, B: GlobalSettings> GlobalSettingsSet for (A, B) {
    type Current = (Arc<A>, Arc<B>);

    fn current() -> Self::Current {
        (current::<A>(), current::<B>())
    }

    fn types(types: &mut Vec<GlobalSettingsType>) {
        A::types(types);
        B::types(types);
    }
}

impl<A: GlobalSettings, B: GlobalSettings, C: GlobalSettings> GlobalSettingsSet for (A, B, C) {
    type Current = (Arc<A>, Arc<B>, Arc<C>);

    fn current() -> Self::Current {
        (current::<A>(), current::<B>(), current::<C>())
    }

    fn types(types: &mut Vec<GlobalSettingsType>) {
        A::types(types);
        B::types(types);
        C::types(types);
    }
}

/// A type of global settings, for the app: its tab, and keeping it between sessions.
#[derive(Clone, Copy)]
pub struct GlobalSettingsType {
    pub name: &'static str,
    type_id: TypeId,
    load: fn(&dyn eframe::Storage),
    save: fn(&mut dyn eframe::Storage),
    ui: fn(&mut egui::Ui, &Colours),
}

impl GlobalSettingsType {
    pub fn of<T: GlobalSettings>() -> Self {
        GlobalSettingsType {
            name: T::NAME,
            type_id: TypeId::of::<T>(),
            load: |storage| {
                if let Some(value) = eframe::get_value::<T>(storage, &key(T::NAME)) {
                    set(value);
                }
            },
            save: |storage| eframe::set_value(storage, &key(T::NAME), &*current::<T>()),
            ui: |ui, colours| {
                let mut value = T::clone(&current::<T>());
                value.ui(ui, colours);
                if value != *current::<T>() {
                    set(value);
                }
            },
        }
    }

    /// Takes what was kept from the last session, if anything.
    pub fn load(&self, storage: &dyn eframe::Storage) {
        (self.load)(storage)
    }

    pub fn save(&self, storage: &mut dyn eframe::Storage) {
        (self.save)(storage)
    }

    /// Its tab.
    pub fn ui(&self, ui: &mut egui::Ui, colours: &Colours) {
        (self.ui)(ui, colours)
    }
}

fn key(name: &str) -> String {
    format!("global settings: {name}")
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Clone, Default, PartialEq, Debug, Serialize, Deserialize)]
    struct Folders(Vec<String>);

    impl GlobalSettings for Folders {
        const NAME: &'static str = "Folders";
        fn ui(&mut self, _ui: &mut egui::Ui, _colours: &Colours) {}
    }

    #[derive(Clone, Default, PartialEq, Debug, Serialize, Deserialize)]
    struct Rate(u32);

    impl GlobalSettings for Rate {
        const NAME: &'static str = "Rate";
        fn ui(&mut self, _ui: &mut egui::Ui, _colours: &Colours) {}
    }

    #[test]
    fn values_are_had_by_type_from_any_thread() {
        assert_eq!(*current::<Rate>(), Rate(0));
        set(Folders(vec!["a".into()]));
        set(Rate(5));
        set(Rate(7));
        let (folders, rate) = std::thread::spawn(<(Folders, Rate)>::current).join().unwrap();
        assert_eq!((*folders).clone(), Folders(vec!["a".into()]));
        assert_eq!(*rate, Rate(7));
    }

    /// Kept as eframe keeps them: by key, as text.
    #[derive(Default)]
    struct Storage(HashMap<String, String>);

    impl eframe::Storage for Storage {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.into(), value);
        }
        fn remove_string(&mut self, key: &str) {
            self.0.remove(key);
        }
        fn flush(&mut self) {}
    }

    #[derive(Clone, Default, PartialEq, Debug, Serialize, Deserialize)]
    struct Kept(u32);

    impl GlobalSettings for Kept {
        const NAME: &'static str = "Kept";
        fn ui(&mut self, _ui: &mut egui::Ui, _colours: &Colours) {}
    }

    #[test]
    fn values_are_kept_between_sessions_by_name() {
        let kept = GlobalSettingsType::of::<Kept>();
        let mut storage = Storage::default();
        kept.load(&storage);
        assert_eq!(*current::<Kept>(), Kept(0), "nothing kept yet");
        set(Kept(3));
        kept.save(&mut storage);
        set(Kept(9));
        kept.load(&storage);
        assert_eq!(*current::<Kept>(), Kept(3));
        assert!(storage.0.contains_key("global settings: Kept"));
    }

    #[derive(Clone, Default, PartialEq, Debug, Serialize, Deserialize)]
    struct Edited(crate::search_folders::SearchFolders);

    impl GlobalSettings for Edited {
        const NAME: &'static str = "Edited";
        fn ui(&mut self, ui: &mut egui::Ui, colours: &Colours) {
            self.0.ui(ui, colours);
        }
    }

    #[test]
    fn what_a_tab_changes_is_the_value_now() {
        use egui_kittest::kittest::Queryable;
        set(Edited(crate::search_folders::SearchFolders(vec!["/a".into(), "/b".into()])));
        let tab = GlobalSettingsType::of::<Edited>();
        let mut harness =
            egui_kittest::Harness::new_ui(move |ui| tab.ui(ui, &crate::standard::themes::THEMES[0].colours));
        harness.run();
        harness.query_all_by_label("Remove").next().unwrap().click();
        harness.run();
        assert_eq!(current::<Edited>().0.0, vec![std::path::PathBuf::from("/b")]);
    }

    #[test]
    fn a_type_named_twice_is_one_tab() {
        let mut types = Vec::new();
        <(Folders, Rate)>::types(&mut types);
        Folders::types(&mut types);
        <()>::types(&mut types);
        assert_eq!(types.iter().map(|t| t.name).collect::<Vec<_>>(), ["Folders", "Rate"]);
    }
}
