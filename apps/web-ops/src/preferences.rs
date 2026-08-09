use leptos::prelude::*;
use lucide_leptos::PanelTop;

const THEME_STORAGE_KEY: &str = "wareboxes.display.theme";
const DENSITY_STORAGE_KEY: &str = "wareboxes.display.density";
const REDUCE_MOTION_STORAGE_KEY: &str = "wareboxes.display.reduce-motion";
const HIDE_NAVIGATION_STORAGE_KEY: &str = "wareboxes.display.hide-navigation";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemePreference {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemePreference {
    const fn storage_value(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    fn from_storage(value: &str) -> Option<Self> {
        match value {
            "system" => Some(Self::System),
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DensityPreference {
    #[default]
    Compact,
    Standard,
}

impl DensityPreference {
    const fn storage_value(self) -> &'static str {
        match self {
            Self::Compact => "compact",
            Self::Standard => "standard",
        }
    }

    fn from_storage(value: &str) -> Option<Self> {
        match value {
            "compact" => Some(Self::Compact),
            "standard" => Some(Self::Standard),
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
pub struct DisplayPreferences {
    theme: RwSignal<ThemePreference>,
    density: RwSignal<DensityPreference>,
    reduce_motion: RwSignal<bool>,
    hide_navigation: RwSignal<bool>,
}

impl DisplayPreferences {
    fn new() -> Self {
        Self {
            theme: RwSignal::new(ThemePreference::default()),
            density: RwSignal::new(DensityPreference::default()),
            reduce_motion: RwSignal::new(false),
            hide_navigation: RwSignal::new(false),
        }
    }

    pub fn theme(self) -> ReadSignal<ThemePreference> {
        self.theme.read_only()
    }

    pub fn density(self) -> ReadSignal<DensityPreference> {
        self.density.read_only()
    }

    pub fn reduce_motion(self) -> ReadSignal<bool> {
        self.reduce_motion.read_only()
    }

    pub fn hide_navigation(self) -> ReadSignal<bool> {
        self.hide_navigation.read_only()
    }

    pub fn set_theme(self, theme: ThemePreference) {
        self.theme.set(theme);
        browser::set_preference(THEME_STORAGE_KEY, theme.storage_value());
        browser::set_root_attribute("data-theme", theme.storage_value());
    }

    pub fn set_density(self, density: DensityPreference) {
        self.density.set(density);
        browser::set_preference(DENSITY_STORAGE_KEY, density.storage_value());
        browser::set_root_attribute("data-density", density.storage_value());
    }

    pub fn set_reduce_motion(self, reduce_motion: bool) {
        self.reduce_motion.set(reduce_motion);
        let value = if reduce_motion { "true" } else { "false" };
        browser::set_preference(REDUCE_MOTION_STORAGE_KEY, value);
        browser::set_root_attribute("data-reduce-motion", value);
    }

    pub fn set_hide_navigation(self, hide_navigation: bool) {
        self.hide_navigation.set(hide_navigation);
        let value = if hide_navigation { "true" } else { "false" };
        browser::set_preference(HIDE_NAVIGATION_STORAGE_KEY, value);
        browser::set_root_attribute("data-navigation-hidden", value);
    }

    fn load_from_browser(self) {
        let theme = browser::preference(THEME_STORAGE_KEY)
            .as_deref()
            .and_then(ThemePreference::from_storage)
            .unwrap_or_default();
        let density = browser::preference(DENSITY_STORAGE_KEY)
            .as_deref()
            .and_then(DensityPreference::from_storage)
            .unwrap_or_default();
        let reduce_motion =
            browser::preference(REDUCE_MOTION_STORAGE_KEY).as_deref() == Some("true");
        let hide_navigation =
            browser::preference(HIDE_NAVIGATION_STORAGE_KEY).as_deref() == Some("true");

        self.theme.set(theme);
        self.density.set(density);
        self.reduce_motion.set(reduce_motion);
        self.hide_navigation.set(hide_navigation);
        browser::set_root_attribute("data-theme", theme.storage_value());
        browser::set_root_attribute("data-density", density.storage_value());
        browser::set_root_attribute(
            "data-reduce-motion",
            if reduce_motion { "true" } else { "false" },
        );
        browser::set_root_attribute(
            "data-navigation-hidden",
            if hide_navigation { "true" } else { "false" },
        );
    }
}

pub fn provide_display_preferences() -> DisplayPreferences {
    let preferences = DisplayPreferences::new();
    provide_context(preferences);

    Effect::new(move || preferences.load_from_browser());

    preferences
}

pub fn use_display_preferences() -> DisplayPreferences {
    expect_context::<DisplayPreferences>()
}

#[component]
pub fn DisplayOptionsMenu() -> impl IntoView {
    let preferences = use_display_preferences();
    let theme = preferences.theme();
    let density = preferences.density();
    let reduce_motion = preferences.reduce_motion();
    let hide_navigation = preferences.hide_navigation();

    view! {
        <details class="display-options">
            <summary
                class="display-options-trigger"
                title="Display options"
                aria-label="Display options"
            >
                <PanelTop size=16/>
                <span>"Display"</span>
            </summary>
            <section class="display-options-popover" aria-label="Display options">
                <header>
                    <strong>"Display options"</strong>
                    <span>"Saved on this device"</span>
                </header>

                <fieldset>
                    <legend>"Theme"</legend>
                    <div class="preference-segments">
                        <label>
                            <input
                                type="radio"
                                name="display-theme"
                                value="system"
                                prop:checked=move || theme.get() == ThemePreference::System
                                on:change=move |_| {
                                    preferences.set_theme(ThemePreference::System);
                                }
                            />
                            <span>"System"</span>
                        </label>
                        <label>
                            <input
                                type="radio"
                                name="display-theme"
                                value="light"
                                prop:checked=move || theme.get() == ThemePreference::Light
                                on:change=move |_| {
                                    preferences.set_theme(ThemePreference::Light);
                                }
                            />
                            <span>"Light"</span>
                        </label>
                        <label>
                            <input
                                type="radio"
                                name="display-theme"
                                value="dark"
                                prop:checked=move || theme.get() == ThemePreference::Dark
                                on:change=move |_| {
                                    preferences.set_theme(ThemePreference::Dark);
                                }
                            />
                            <span>"Dark"</span>
                        </label>
                    </div>
                </fieldset>

                <fieldset>
                    <legend>"Density"</legend>
                    <div class="preference-segments">
                        <label>
                            <input
                                type="radio"
                                name="display-density"
                                value="compact"
                                prop:checked=move || density.get() == DensityPreference::Compact
                                on:change=move |_| {
                                    preferences.set_density(DensityPreference::Compact);
                                }
                            />
                            <span>"Compact"</span>
                        </label>
                        <label>
                            <input
                                type="radio"
                                name="display-density"
                                value="standard"
                                prop:checked=move || density.get() == DensityPreference::Standard
                                on:change=move |_| {
                                    preferences.set_density(DensityPreference::Standard);
                                }
                            />
                            <span>"Standard"</span>
                        </label>
                    </div>
                </fieldset>

                <label class="preference-toggle">
                    <span>
                        <strong>"Reduce motion"</strong>
                        <small>"Minimize interface animation"</small>
                    </span>
                    <input
                        type="checkbox"
                        role="switch"
                        prop:checked=move || reduce_motion.get()
                        on:change=move |event| {
                            preferences.set_reduce_motion(event_target_checked(&event));
                        }
                    />
                    <i aria-hidden="true"></i>
                </label>

                <label class="preference-toggle">
                    <span>
                        <strong>"Hide navigation"</strong>
                        <small>"Use the full width for operations"</small>
                    </span>
                    <input
                        type="checkbox"
                        role="switch"
                        prop:checked=move || hide_navigation.get()
                        on:change=move |event| {
                            preferences.set_hide_navigation(event_target_checked(&event));
                        }
                    />
                    <i aria-hidden="true"></i>
                </label>
            </section>
        </details>
    }
}

#[cfg(target_arch = "wasm32")]
mod browser {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen]
    extern "C" {
        #[wasm_bindgen(js_namespace = localStorage, js_name = getItem, catch)]
        fn local_storage_get(key: &str) -> Result<JsValue, JsValue>;

        #[wasm_bindgen(js_namespace = localStorage, js_name = setItem, catch)]
        fn local_storage_set(key: &str, value: &str) -> Result<(), JsValue>;
    }

    pub fn preference(key: &str) -> Option<String> {
        local_storage_get(key).ok()?.as_string()
    }

    pub fn set_preference(key: &str, value: &str) {
        drop(local_storage_set(key, value));
    }

    pub fn set_root_attribute(name: &str, value: &str) {
        let Some(root) = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.document_element())
        else {
            return;
        };
        drop(root.set_attribute(name, value));
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod browser {
    pub fn preference(_key: &str) -> Option<String> {
        None
    }

    pub fn set_preference(_key: &str, _value: &str) {}

    pub fn set_root_attribute(_name: &str, _value: &str) {}
}
