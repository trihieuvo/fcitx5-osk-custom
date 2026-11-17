use std::{
    collections::HashMap,
    fmt::{Display, Formatter, Result as FmtResult},
    str::FromStr,
    sync::Arc,
    time::Duration,
};

use iced::Task;
use strum::IntoEnumIterator;

use crate::{
    app::{KeyboardNotification, Message},
    config::{
        Config, ConfigManager, IndicatorDisplay, KeySound, Placement, QuickActionBarState,
    },
    dbus::server::ImPanelEvent,
    layout::KLength,
    state::{ImEvent, LayoutEvent, StateExtractor, StoreEvent, ThemeEvent, WindowManagerEvent},
    window::WindowManagerMode,
};

macro_rules! on_update_event {
    ($event:ident, $config:expr, $(@$variant:ident => {$eq:expr, $set:ident $(, $message_cb:expr)?}),*$(,)? $($pat: pat => $raw_expr: expr),* $(,)?) => {
        match $event {
            $(UpdateConfigEvent::$variant(v) => {
                if $eq($config, &v) {
                    (false, None)
                } else {
                    $config.$set(v.clone());
                    #[allow(unused_mut, unused_assignments)]
                    let mut message = None;
                    $( message = Some($message_cb(v)); )?
                    (true, message)
                }
            },)*
            $($pat => $raw_expr,)*
        }
    }
}

macro_rules! config_eq {
    ($field:tt) => {
        |c: &Config, v| c.$field().eq(v)
    };
}

macro_rules! option_config_eq {
    ($field:tt) => {
        |c: &Config, v| c.$field().filter(|f| (*f).eq(v)).is_some()
    };
}

#[derive(Debug)]
pub struct ValueAndDescription<T> {
    pub value: T,
    pub desc: String,
}

impl<T> Display for ValueAndDescription<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.write_str(&self.desc)
    }
}

impl<T> Clone for ValueAndDescription<T>
where
    T: Clone,
{
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            desc: self.desc.clone(),
        }
    }
}

impl<T> PartialEq for ValueAndDescription<T>
where
    T: PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        self.value.eq(&other.value)
    }
}

pub struct StepDesc<T> {
    cur_value: fn(&dyn StateExtractor) -> T,
    step: fn(&dyn StateExtractor) -> T,
    on_increased: fn(&dyn StateExtractor, T, T) -> Option<Message>,
    on_decreased: fn(&dyn StateExtractor, T, T) -> Option<Message>,
}

impl<T> StepDesc<T> {
    pub fn cur_value(&self, state: &dyn StateExtractor) -> T {
        (self.cur_value)(state)
    }

    pub fn on_increased(&self, state: &dyn StateExtractor) -> Option<Message> {
        (self.on_increased)(state, self.cur_value(state), (self.step)(state))
    }

    pub fn on_decreased(&self, state: &dyn StateExtractor) -> Option<Message> {
        (self.on_decreased)(state, self.cur_value(state), (self.step)(state))
    }
}

pub struct RangeDesc<T> {
    init_value: fn(&dyn StateExtractor) -> T,
    min_value: fn(&dyn StateExtractor) -> T,
    max_value: fn(&dyn StateExtractor) -> T,
    /// The value stored in the restore point, if any.
    restore_value: fn(&dyn StateExtractor, &Config) -> T,
    step: fn(&dyn StateExtractor) -> T,
    format: fn(&dyn StateExtractor, T) -> String,
    is_enabled: fn(&dyn StateExtractor) -> bool,
    check: fn(&dyn StateExtractor, value: T) -> bool,
    on_changed: fn(String) -> Message,
}

impl<T> RangeDesc<T>
where
    T: FromStr + PartialEq + Copy,
{
    pub fn init_value(&self, state: &dyn StateExtractor) -> T {
        (self.init_value)(state)
    }

    pub fn cur_value(&self, field: &Field, state: &dyn StateExtractor) -> T {
        let init_value = (self.init_value)(state);
        if let Some((last_init_value, value)) = state.config_temp_text(field.id())
            && last_init_value
                .parse::<T>()
                .ok()
                .filter(|l| *l == init_value)
                .is_some()
        {
            return value.parse::<T>().unwrap_or(init_value);
        }
        init_value
    }

    pub fn min_value(&self, state: &dyn StateExtractor) -> T {
        (self.min_value)(state)
    }

    pub fn max_value(&self, state: &dyn StateExtractor) -> T {
        (self.max_value)(state)
    }

    /// A message restoring the value of the restore point, if the current one differs.
    pub fn restore_message(&self, field: &Field, state: &dyn StateExtractor) -> Option<Message>
    where
        T: ToString + Into<f64>,
    {
        if !self.is_enabled(state) {
            return None;
        }
        let restore = (self.restore_value)(state, state.restore_point()?);
        // a restore point from another screen may not fit this one
        if !self.check(state, restore) {
            return None;
        }
        let cur: f64 = self.cur_value(field, state).into();
        let restore_f: f64 = restore.into();
        // the width is derived from the layout, so it is only approximately the stored one
        if (cur - restore_f).abs() < f64::max(1e-4, restore_f.abs() * 1e-3) {
            return None;
        }
        Some(Message::from(UpdateConfigEvent::Restore {
            key: field.id().to_string(),
            message: Box::new((self.on_changed)(restore.to_string())),
        }))
    }

    pub fn step(&self, state: &dyn StateExtractor) -> T {
        (self.step)(state)
    }

    pub fn format(&self, state: &dyn StateExtractor, t: T) -> String {
        (self.format)(state, t)
    }

    pub fn is_enabled(&self, state: &dyn StateExtractor) -> bool {
        (self.is_enabled)(state)
    }

    pub fn check(&self, state: &dyn StateExtractor, value: T) -> bool {
        (self.check)(state, value)
    }

    pub fn on_changed_cb(&self) -> fn(String) -> Message {
        self.on_changed
    }
}

pub struct OwnedEnumDesc<T> {
    cur_value: fn(&dyn StateExtractor) -> Option<T>,
    variants: Vec<T>,
    is_enabled: fn(&dyn StateExtractor) -> bool,
    on_selected: fn(&dyn StateExtractor, T) -> Message,
}

impl<T> OwnedEnumDesc<T> {
    pub fn cur_value(&self, state: &dyn StateExtractor) -> Option<T> {
        (self.cur_value)(state)
    }

    pub fn variants(&self) -> &[T] {
        &self.variants
    }

    pub fn is_enabled(&self, state: &dyn StateExtractor) -> bool {
        (self.is_enabled)(state)
    }

    pub fn on_selected(&self, state: &dyn StateExtractor, selected: T) -> Message {
        (self.on_selected)(state, selected)
    }
}

pub struct EnumDesc<T> {
    cur_value: fn(&dyn StateExtractor) -> Option<&T>,
    variants: Vec<T>,
    is_enabled: fn(&dyn StateExtractor) -> bool,
    on_selected: fn(&dyn StateExtractor, T) -> Message,
}

impl<T> EnumDesc<T> {
    pub fn cur_value<'a>(&self, state: &'a dyn StateExtractor) -> Option<&'a T> {
        (self.cur_value)(state)
    }

    pub fn variants(&self) -> &[T] {
        &self.variants
    }

    pub fn is_enabled(&self, state: &dyn StateExtractor) -> bool {
        (self.is_enabled)(state)
    }

    pub fn on_selected(&self, state: &dyn StateExtractor, selected: T) -> Message {
        (self.on_selected)(state, selected)
    }
}

pub struct DynamicEnumDesc<T> {
    #[allow(clippy::type_complexity)]
    variants_and_selected:
        fn(&dyn StateExtractor) -> (Vec<ValueAndDescription<T>>, Option<ValueAndDescription<T>>),
    is_enabled: fn(&dyn StateExtractor) -> bool,
    on_selected: fn(&dyn StateExtractor, ValueAndDescription<T>) -> Message,
}

impl<T> DynamicEnumDesc<T> {
    pub fn variants_and_selected(
        &self,
        state: &dyn StateExtractor,
    ) -> (Vec<ValueAndDescription<T>>, Option<ValueAndDescription<T>>) {
        (self.variants_and_selected)(state)
    }

    pub fn is_enabled(&self, state: &dyn StateExtractor) -> bool {
        (self.is_enabled)(state)
    }

    pub fn on_selected(
        &self,
        state: &dyn StateExtractor,
        selected: ValueAndDescription<T>,
    ) -> Message {
        (self.on_selected)(state, selected)
    }
}

/// TODO Actually, this doesn't work because the keyboard ui doesn't accept input, you can't type text. How funny I am!
pub struct TextDesc {
    placeholder: fn(&Field, &dyn StateExtractor) -> Option<String>,
    init_value: fn(&dyn StateExtractor) -> Option<String>,
    is_enabled: fn(&dyn StateExtractor) -> bool,
    submit_message: fn(String) -> Message,
}

impl TextDesc {
    pub fn placeholder(&self, field: &Field, state: &dyn StateExtractor) -> Option<String> {
        (self.placeholder)(field, state)
    }

    pub fn cur_value(&self, field: &Field, state: &dyn StateExtractor) -> Option<String> {
        let init_value = (self.init_value)(state);
        if let Some((last_init_value, value)) = state.config_temp_text(field.id()) {
            if last_init_value.is_empty() && init_value.is_none() {
                return Some(value.to_string());
            }
            if init_value
                .as_ref()
                .filter(|v| *v == last_init_value)
                .is_some()
            {
                return Some(value.to_string());
            }
        }
        init_value
    }

    pub fn on_input_maybe(
        &self,
        field: &Field,
        state: &dyn StateExtractor,
    ) -> Option<Box<dyn Fn(String) -> Message>> {
        if !(self.is_enabled)(state) {
            return None;
        }

        let init_value = (self.init_value)(state).unwrap_or_default();
        let key = field.id().to_string();

        let handle = move |value| {
            Message::from(UpdateConfigEvent::ChangeTempText {
                key: key.clone(),
                init_value: init_value.clone(),
                value,
            })
        };
        Some(Box::new(handle))
    }

    pub fn on_paste_maybe(
        &self,
        field: &Field,
        state: &dyn StateExtractor,
    ) -> Option<Box<dyn Fn(String) -> Message>> {
        self.on_input_maybe(field, state)
    }

    pub fn on_submit_maybe(&self, field: &Field, state: &dyn StateExtractor) -> Option<Message> {
        if !(self.is_enabled)(state) {
            return None;
        }

        let init_value = (self.init_value)(state).unwrap_or_default();
        let key = field.id().to_string();

        Some(Message::from(UpdateConfigEvent::SubmitTempText {
            key,
            init_value,
            producer: self.submit_message,
        }))
    }
}

pub struct BoolDesc {
    restore_value: Option<fn(&Config) -> bool>,
    cur_value: fn(&dyn StateExtractor) -> bool,
    is_enabled: fn(&dyn StateExtractor) -> bool,
    on_changed: fn(&dyn StateExtractor, bool) -> Message,
}

impl BoolDesc {
    pub fn cur_value(&self, state: &dyn StateExtractor) -> bool {
        (self.cur_value)(state)
    }

    pub fn is_enabled(&self, state: &dyn StateExtractor) -> bool {
        (self.is_enabled)(state)
    }

    pub fn on_changed(&self, state: &dyn StateExtractor, value: bool) -> Message {
        (self.on_changed)(state, value)
    }

    pub fn restore_message(&self, state: &dyn StateExtractor) -> Option<Message> {
        let restore = (self.restore_value?)(state.restore_point()?);
        (restore != self.cur_value(state) && self.is_enabled(state)).then(|| {
            self.on_changed(state, restore)
        })
    }
}

pub struct MultiSelectionDesc<T> {
    variants: fn(&dyn StateExtractor) -> Vec<ValueAndDescription<T>>,
    selecteds: fn(&dyn StateExtractor) -> Vec<ValueAndDescription<T>>,
    on_changed: fn(&dyn StateExtractor, selections: &[ValueAndDescription<T>]) -> Message,
    is_enabled: fn(&dyn StateExtractor) -> bool,
}

impl<T> MultiSelectionDesc<T> {
    pub fn variants(&self, state: &dyn StateExtractor) -> Vec<ValueAndDescription<T>> {
        (self.variants)(state)
    }

    pub fn selecteds(&self, state: &dyn StateExtractor) -> Vec<ValueAndDescription<T>> {
        (self.selecteds)(state)
    }

    pub fn on_changed(
        &self,
        state: &dyn StateExtractor,
        selections: &[ValueAndDescription<T>],
    ) -> Message {
        (self.on_changed)(state, selections)
    }

    pub fn is_enabled(&self, state: &dyn StateExtractor) -> bool {
        (self.is_enabled)(state)
    }
}

pub enum FieldType {
    StepU32(StepDesc<u32>),
    RangeF32(RangeDesc<f32>),
    OwnedEnumPlacement(OwnedEnumDesc<Placement>),
    OwnedEnumIndicatorDisplay(OwnedEnumDesc<IndicatorDisplay>),
    OwnedEnumQuickActionBarState(OwnedEnumDesc<QuickActionBarState>),
    OwnedEnumKeySound(OwnedEnumDesc<KeySound>),
    EnumString(EnumDesc<String>),
    DynamicEnumString(DynamicEnumDesc<String>),
    Text(TextDesc),
    Bool(BoolDesc),
    MultiSelectionString(MultiSelectionDesc<String>),
}

impl From<StepDesc<u32>> for FieldType {
    fn from(value: StepDesc<u32>) -> Self {
        Self::StepU32(value)
    }
}

impl From<RangeDesc<f32>> for FieldType {
    fn from(value: RangeDesc<f32>) -> Self {
        Self::RangeF32(value)
    }
}

impl From<OwnedEnumDesc<Placement>> for FieldType {
    fn from(value: OwnedEnumDesc<Placement>) -> Self {
        Self::OwnedEnumPlacement(value)
    }
}

impl From<OwnedEnumDesc<IndicatorDisplay>> for FieldType {
    fn from(value: OwnedEnumDesc<IndicatorDisplay>) -> Self {
        Self::OwnedEnumIndicatorDisplay(value)
    }
}

impl From<OwnedEnumDesc<KeySound>> for FieldType {
    fn from(value: OwnedEnumDesc<KeySound>) -> Self {
        Self::OwnedEnumKeySound(value)
    }
}

impl From<OwnedEnumDesc<QuickActionBarState>> for FieldType {
    fn from(value: OwnedEnumDesc<QuickActionBarState>) -> Self {
        Self::OwnedEnumQuickActionBarState(value)
    }
}

impl From<EnumDesc<String>> for FieldType {
    fn from(value: EnumDesc<String>) -> Self {
        Self::EnumString(value)
    }
}

impl From<DynamicEnumDesc<String>> for FieldType {
    fn from(value: DynamicEnumDesc<String>) -> Self {
        Self::DynamicEnumString(value)
    }
}

impl From<TextDesc> for FieldType {
    fn from(value: TextDesc) -> Self {
        Self::Text(value)
    }
}

impl From<BoolDesc> for FieldType {
    fn from(value: BoolDesc) -> Self {
        Self::Bool(value)
    }
}

impl From<MultiSelectionDesc<String>> for FieldType {
    fn from(value: MultiSelectionDesc<String>) -> Self {
        Self::MultiSelectionString(value)
    }
}

pub struct Field {
    name: &'static str,
    id: &'static str,
    typ: FieldType,
    should_display: fn(&dyn StateExtractor) -> bool,
}

impl Field {
    /// A message putting this field back to the value of the restore point, if it differs.
    pub fn restore_message(&self, state: &dyn StateExtractor) -> Option<Message> {
        match &self.typ {
            FieldType::RangeF32(desc) => desc.restore_message(self, state),
            FieldType::Bool(desc) => desc.restore_message(state),
            _ => None,
        }
    }

    pub fn name(&self) -> &str {
        self.name
    }

    pub fn id(&self) -> &str {
        self.id
    }

    pub fn typ(&self) -> &FieldType {
        &self.typ
    }

    pub fn should_display(&self, state: &dyn StateExtractor) -> bool {
        (self.should_display)(state)
    }
}

pub struct ConfigState {
    config_manager: ConfigManager,
    updatable_fields: Vec<Field>,
    temp_texts: HashMap<String, (String, String)>,
}

impl ConfigState {
    pub fn new(config_manager: ConfigManager) -> Self {
        Self {
            config_manager,
            updatable_fields: vec![
                Field {
                    name: "Width",
                    id: "width",
                    typ: RangeDesc::<f32> {
                        init_value: |state| state.keyboard_window_size().width.val(),
                        min_value: |_state| 360.,
                        max_value: |state| state.screen_size().width,
                        restore_value: |state, c| if state.is_portrait() { c.portrait_width().val() } else { c.landscape_width().val() },
                        step: |_state| 1.,
                        format: |_state, v| format!("{:.1}", v),
                        is_enabled: |_state| true,
                        check: |state, value| value >= 360. && value <= state.screen_size().width,
                        on_changed: |value| {
                            if let Ok(v) = value.parse::<f32>() {
                                Message::from(WindowManagerEvent::UpdateWidth(v.into()))
                            } else {
                                KeyboardNotification::Error(Arc::new(anyhow::anyhow!(
                                    "{value} is not a f32"
                                )))
                                .into()
                            }
                        },
                    }
                    .into(),
                    should_display: default_should_display,
                },
                Field {
                    name: "Key height scale",
                    id: "key_height_scale",
                    typ: RangeDesc::<f32> {
                        init_value: |state| state.config().key_height_scale(),
                        min_value: |_state| 0.5,
                        max_value: |_state| 2.,
                        restore_value: |_state, c| c.key_height_scale(),
                        step: |_state| 0.05,
                        format: |_state, v| format!("{:.2}", v),
                        is_enabled: |_state| true,
                        check: |_state, value| (0.5..=2.).contains(&value),
                        on_changed: |value| {
                            if let Ok(v) = value.parse::<f32>() {
                                Message::from(UpdateConfigEvent::KeyHeightScale(v))
                            } else {
                                KeyboardNotification::Error(Arc::new(anyhow::anyhow!(
                                    "{value} is not a f32"
                                )))
                                .into()
                            }
                        },
                    }
                    .into(),
                    should_display: default_should_display,
                },
                Field {
                    name: "Key spacing scale",
                    id: "key_spacing_scale",
                    typ: RangeDesc::<f32> {
                        init_value: |state| state.config().key_spacing_scale(),
                        min_value: |_state| 0.5,
                        max_value: |_state| 2.,
                        restore_value: |_state, c| c.key_spacing_scale(),
                        step: |_state| 0.05,
                        format: |_state, v| format!("{:.2}", v),
                        is_enabled: |_state| true,
                        check: |_state, value| (0.5..=2.).contains(&value),
                        on_changed: |value| {
                            if let Ok(v) = value.parse::<f32>() {
                                Message::from(UpdateConfigEvent::KeySpacingScale(v))
                            } else {
                                KeyboardNotification::Error(Arc::new(anyhow::anyhow!(
                                    "{value} is not a f32"
                                )))
                                .into()
                            }
                        },
                    }
                    .into(),
                    should_display: default_should_display,
                },
                Field {
                    name: "Key corner radius",
                    id: "key_border_radius",
                    typ: RangeDesc::<f32> {
                        init_value: |_state| crate::widget::button::border_radius(),
                        min_value: |_state| 0.,
                        max_value: |_state| 30.,
                        restore_value: |_state, c| c.key_border_radius(),
                        step: |_state| 1.,
                        format: |_state, v| format!("{:.1}", v),
                        is_enabled: |_state| true,
                        check: |_state, value| (0. ..=30.).contains(&value),
                        on_changed: |value| {
                            if let Ok(v) = value.parse::<f32>() {
                                Message::from(UpdateConfigEvent::KeyBorderRadius(v))
                            } else {
                                KeyboardNotification::Error(Arc::new(anyhow::anyhow!(
                                    "{value} is not a f32"
                                )))
                                .into()
                            }
                        },
                    }
                    .into(),
                    should_display: default_should_display,
                },
                Field {
                    name: "Key press animation",
                    id: "key_animation",
                    typ: BoolDesc {
                        restore_value: Some(|c| c.key_animation()),
                        cur_value: |state| state.config().key_animation(),
                        is_enabled: |_state| true,
                        on_changed: |_, v| Message::from(UpdateConfigEvent::KeyAnimation(v)),
                    }
                    .into(),
                    should_display: default_should_display,
                },
                Field {
                    name: "Key preview popup",
                    id: "key_preview",
                    typ: BoolDesc {
                        restore_value: Some(|c| c.key_preview()),
                        cur_value: |state| state.config().key_preview(),
                        is_enabled: |_state| true,
                        on_changed: |_, v| Message::from(UpdateConfigEvent::KeyPreview(v)),
                    }
                    .into(),
                    should_display: default_should_display,
                },
                Field {
                    name: "Key sound",
                    id: "key_sound",
                    typ: OwnedEnumDesc::<KeySound> {
                        cur_value: |state| Some(state.config().key_sound()),
                        variants: KeySound::iter().collect(),
                        is_enabled: |_state| true,
                        on_selected: |_, s| Message::from(UpdateConfigEvent::KeySound(s)),
                    }
                    .into(),
                    should_display: default_should_display,
                },
                Field {
                    name: "Key sound volume",
                    id: "key_sound_volume",
                    typ: RangeDesc::<f32> {
                        init_value: |state| sane_volume(state.config().key_sound_volume()),
                        min_value: |_state| 0.,
                        max_value: |_state| 1.,
                        restore_value: |_state, c| sane_volume(c.key_sound_volume()),
                        step: |_state| 0.05,
                        format: |_state, v| format!("{:.0}%", v * 100.),
                        is_enabled: |state| state.config().key_sound() != KeySound::Off,
                        check: |_state, value| (0. ..=1.).contains(&value),
                        on_changed: |value| {
                            if let Ok(v) = value.parse::<f32>() {
                                Message::from(UpdateConfigEvent::KeySoundVolume(v))
                            } else {
                                KeyboardNotification::Error(Arc::new(anyhow::anyhow!(
                                    "{value} is not a f32"
                                )))
                                .into()
                            }
                        },
                    }
                    .into(),
                    should_display: default_should_display,
                },
                Field {
                    name: "Placement",
                    id: "placement",
                    typ: OwnedEnumDesc::<Placement> {
                        cur_value: |state| Some(state.placement()),
                        variants: Placement::iter().collect(),
                        is_enabled: |state| {
                            state.window_manager_mode() == WindowManagerMode::Normal
                        },
                        on_selected: |_, p| Message::from(WindowManagerEvent::UpdatePlacement(p)),
                    }
                    .into(),
                    should_display: default_should_display,
                },
                Field {
                    name: "Indicator Display",
                    id: "indicator_display",
                    typ: OwnedEnumDesc::<IndicatorDisplay> {
                        cur_value: |state| Some(state.indicator_display()),
                        variants: IndicatorDisplay::iter().collect(),
                        is_enabled: |state| {
                            state.window_manager_mode() == WindowManagerMode::Normal
                        },
                        on_selected: |_, d| {
                            Message::from(WindowManagerEvent::UpdateIndicatorDisplay(d))
                        },
                    }
                    .into(),
                    should_display: default_should_display,
                },
                Field {
                    name: "Manual Mode",
                    id: "manual_mode",
                    typ: BoolDesc {
                        restore_value: Some(|c| c.manual_mode()),
                        cur_value: |state| state.config().manual_mode(),
                        is_enabled: |_state| true,
                        on_changed: |_, v| Message::from(UpdateConfigEvent::ManualMode(v)),
                    }
                    .into(),
                    should_display: default_should_display,
                },
                Field {
                    name: "Manual Mode(Session)",
                    id: "session_manual_mode",
                    typ: BoolDesc {
                        restore_value: None,
                        cur_value: |state| state.session_manual_mode().unwrap_or(false),
                        is_enabled: |_| false,
                        on_changed: |_, _| Message::Nothing,
                    }
                    .into(),
                    should_display: |state| state.session_manual_mode().is_some(),
                },
                Field {
                    name: "Ignore Fcitx Show",
                    id: "ignore_fcitx_show",
                    typ: BoolDesc {
                        restore_value: Some(|c| c.ignore_fcitx_show()),
                        cur_value: |state| state.config().ignore_fcitx_show(),
                        is_enabled: |_state| true,
                        on_changed: |_, v| Message::from(UpdateConfigEvent::IgnoreFcitxShow(v)),
                    }
                    .into(),
                    should_display: default_should_display,
                },
                Field {
                    name: "Auto show on focus",
                    id: "auto_show_on_focus",
                    typ: BoolDesc {
                        restore_value: Some(|c| c.auto_show_on_focus()),
                        cur_value: |state| state.config().auto_show_on_focus(),
                        is_enabled: |_state| true,
                        on_changed: |_, v| Message::from(UpdateConfigEvent::AutoShowOnFocus(v)),
                    }
                    .into(),
                    should_display: default_should_display,
                },
                Field {
                    name: "Auto hide on blur",
                    id: "auto_hide_on_blur",
                    typ: BoolDesc {
                        restore_value: Some(|c| c.auto_hide_on_blur()),
                        cur_value: |state| state.config().auto_hide_on_blur(),
                        is_enabled: |_state| true,
                        on_changed: |_, v| Message::from(UpdateConfigEvent::AutoHideOnBlur(v)),
                    }
                    .into(),
                    should_display: default_should_display,
                },
                Field {
                    name: "Auto hide delay (ms)",
                    id: "auto_hide_delay",
                    typ: RangeDesc::<f32> {
                        init_value: |state| state.config().auto_hide_delay().as_millis() as f32,
                        min_value: |_state| 0.,
                        max_value: |_state| 2000.,
                        restore_value: |_state, c| c.auto_hide_delay().as_millis() as f32,
                        step: |_state| 1.,
                        format: |_state, v| format!("{:.0}", v),
                        is_enabled: |state| state.config().auto_hide_on_blur(),
                        check: |_state, value| (0. ..=2000.).contains(&value),
                        on_changed: |value| {
                            if let Ok(v) = value.parse::<f32>() {
                                Message::from(UpdateConfigEvent::AutoHideDelay(
                                    Duration::from_millis(v as u64),
                                ))
                            } else {
                                KeyboardNotification::Error(Arc::new(anyhow::anyhow!(
                                    "{value} is not a f32"
                                )))
                                .into()
                            }
                        },
                    }
                    .into(),
                    should_display: default_should_display,
                },
                Field {
                    name: "Auto show: touch only",
                    id: "auto_show_touch_only",
                    typ: BoolDesc {
                        restore_value: Some(|c| c.auto_show_touch_only()),
                        cur_value: |state| state.config().auto_show_touch_only(),
                        is_enabled: |state| {
                            state.config().auto_show_on_focus()
                                || state.config().auto_hide_on_blur()
                        },
                        on_changed: |_, v| Message::from(UpdateConfigEvent::AutoShowTouchOnly(v)),
                    }
                    .into(),
                    should_display: default_should_display,
                },
                Field {
                    name: "Quick Action Bar",
                    id: "quick_action_bar_state",
                    typ: OwnedEnumDesc::<QuickActionBarState> {
                        cur_value: |state| Some(state.config().quick_action_bar_state()),
                        variants: QuickActionBarState::iter().collect(),
                        is_enabled: |_state| true,
                        on_selected: |_, s| {
                            Message::from(UpdateConfigEvent::QuickActionBarState(s))
                        },
                    }
                    .into(),
                    should_display: default_should_display,
                },
                Field {
                    name: "Custom Actions",
                    id: "custom_actions",
                    typ: MultiSelectionDesc::<String> {
                        variants: |state: &dyn StateExtractor| {
                            let mut variants: Vec<_> = state
                                .store()
                                .custom_actions()
                                .iter()
                                .map(|ca| ValueAndDescription {
                                    value: ca.name().clone(),
                                    desc: ca.name().clone(),
                                })
                                .collect();
                            variants.sort_unstable_by(|v1, v2| v1.value.cmp(&v2.value));
                            variants
                        },
                        selecteds: |state| {
                            state
                                .config()
                                .custom_actions()
                                .iter()
                                .map(|s| ValueAndDescription {
                                    value: s.clone(),
                                    desc: s.clone(),
                                })
                                .collect()
                        },
                        on_changed: |_state, selections| {
                            let custom_actions =
                                selections.iter().map(|v| v.value.clone()).collect();
                            Message::from(UpdateConfigEvent::CustomActions(Arc::new(
                                custom_actions,
                            )))
                        },
                        is_enabled: |_state| true,
                    }
                    .into(),
                    should_display: default_should_display,
                },
                Field {
                    name: "Dark Theme",
                    id: "dark_theme",
                    typ: DynamicEnumDesc::<String> {
                        variants_and_selected: |state| {
                            let mut theme_names = vec![];
                            for theme_name in state.theme_names() {
                                if theme_name != "Auto" {
                                    theme_names.push(ValueAndDescription {
                                        value: theme_name.clone(),
                                        desc: theme_name.clone(),
                                    });
                                }
                            }
                            (
                                theme_names,
                                state
                                    .config()
                                    .dark_theme()
                                    .map(|theme_name| ValueAndDescription {
                                        value: theme_name.clone(),
                                        desc: theme_name.clone(),
                                    }),
                            )
                        },
                        is_enabled: |_| true,
                        on_selected: |_, d| Message::from(UpdateConfigEvent::DarkTheme(d.value)),
                    }
                    .into(),
                    should_display: default_should_display,
                },
                Field {
                    name: "Light Theme",
                    id: "light_theme",
                    typ: DynamicEnumDesc::<String> {
                        variants_and_selected: |state| {
                            let mut theme_names = vec![];
                            for theme_name in state.theme_names() {
                                if theme_name != "Auto" {
                                    theme_names.push(ValueAndDescription {
                                        value: theme_name.clone(),
                                        desc: theme_name.clone(),
                                    });
                                }
                            }
                            (
                                theme_names,
                                state.config().light_theme().map(|theme_name| {
                                    ValueAndDescription {
                                        value: theme_name.clone(),
                                        desc: theme_name.clone(),
                                    }
                                }),
                            )
                        },
                        is_enabled: |_| true,
                        on_selected: |_, d| Message::from(UpdateConfigEvent::LightTheme(d.value)),
                    }
                    .into(),
                    should_display: default_should_display,
                },
                preferred_output_name_field(),
                preferred_output_name_custom_field(),
            ],
            temp_texts: Default::default(),
        }
    }

    pub fn config(&self) -> &Config {
        self.config_manager.as_ref()
    }

    pub fn restore_point(&self) -> Option<&Config> {
        self.config_manager.restore_point()
    }

    pub fn updatable_fields(&self) -> &[Field] {
        &self.updatable_fields
    }

    pub fn refresh(&mut self) {
        // clear temp values if needed
        self.temp_texts.clear();
    }

    pub fn temp_text(&self, key: &str) -> Option<(&str, &str)> {
        self.temp_texts
            .get(key)
            .map(|(init_value, value)| (init_value.as_str(), value.as_str()))
    }

    pub fn on_update_event(&mut self, event: UpdateConfigEvent) -> Task<Message> {
        let config = self.config_manager.as_mut();
        let (updated, message) = on_update_event!(
            event,
            config,
            @LandscapeWidth => {
                config_eq!(landscape_width),
                set_landscape_width,
                |_| Message::from(ImEvent::ResetCandidateCursor)
            },
            @PortraitWidth => {
                config_eq!(portrait_width),
                set_portrait_width,
                |_| Message::from(ImEvent::ResetCandidateCursor)
            },
            @Placement => {config_eq!(placement), set_placement},
            @IndicatorDisplay => {config_eq!(indicator_display), set_indicator_display},
            @Theme => {
                config_eq!(theme),
                set_theme,
                |_| Message::from(ThemeEvent::Updated)
            },
            @KeyHeightScale => {
                config_eq!(key_height_scale),
                set_key_height_scale,
                |v| {
                    crate::layout::set_height_scale(v);
                    Message::from(WindowManagerEvent::RelayoutKeyboard)
                }
            },
            @KeySpacingScale => {
                config_eq!(key_spacing_scale),
                set_key_spacing_scale,
                |v| {
                    crate::layout::set_spacing_scale(v);
                    Message::from(WindowManagerEvent::RelayoutKeyboard)
                }
            },
            @KeyBorderRadius => {
                config_eq!(key_border_radius),
                set_key_border_radius,
                |v| {
                    crate::widget::button::set_border_radius(v);
                    Message::Nothing
                }
            },
            @KeyAnimation => {
                config_eq!(key_animation),
                set_key_animation,
                |v| {
                    crate::widget::button::set_animation_enabled(v);
                    Message::Nothing
                }
            },
            @KeyPreview => {
                config_eq!(key_preview),
                set_key_preview,
                |v| {
                    crate::widget::button::set_preview_enabled(v);
                    Message::Nothing
                }
            },
            @KeySound => {
                config_eq!(key_sound),
                set_key_sound,
                |v| {
                    crate::sound::set_kind(v);
                    crate::sound::play_key();
                    Message::Nothing
                }
            },
            @KeySoundVolume => {
                config_eq!(key_sound_volume),
                set_key_sound_volume,
                |v| {
                    crate::sound::set_volume(v);
                    crate::sound::play_key();
                    Message::Nothing
                }
            },
            @DarkTheme => {option_config_eq!(dark_theme), set_dark_theme},
            @LightTheme => {option_config_eq!(light_theme), set_light_theme},
            @PreferredOutputName => {
                option_config_eq!(preferred_output_name),
                set_preferred_output_name,
                |v| Message::from(WindowManagerEvent::UpdatePreferredOutputName(v))
            },
            @ManualMode => {
                config_eq!(manual_mode),
                set_manual_mode,
                |v| Message::from(ImPanelEvent::UpdateManualMode(v))
            },
            @IgnoreFcitxShow => {
                config_eq!(ignore_fcitx_show),
                set_ignore_fcitx_show,
                |_v| Message::Nothing
            },
            @AutoShowOnFocus => {
                config_eq!(auto_show_on_focus),
                set_auto_show_on_focus,
                |v| {
                    if v {
                        crate::input_activity::ensure_started();
                    }
                    Message::Nothing
                }
            },
            @AutoHideOnBlur => {
                config_eq!(auto_hide_on_blur),
                set_auto_hide_on_blur,
                |v| {
                    if v {
                        crate::input_activity::ensure_started();
                    }
                    Message::Nothing
                }
            },
            @AutoHideDelay => {
                config_eq!(auto_hide_delay),
                set_auto_hide_delay,
                |_| Message::Nothing
            },
            @AutoShowTouchOnly => {
                config_eq!(auto_show_touch_only),
                set_auto_show_touch_only,
                |v| {
                    if v {
                        crate::input_activity::ensure_started();
                    }
                    Message::Nothing
                }
            },
            @QuickActionBarState => {
                config_eq!(quick_action_bar_state),
                set_quick_action_bar_state,
                |v| Message::from(LayoutEvent::UpdateQuickActionBarState(v))
            },
            @CustomActions => {
                |c: &Config, v: &Arc<Vec<String>>| c.custom_actions() == v.as_ref(),
                set_custom_actions,
                |v| Message::from(StoreEvent::UpdateCustomActions(v))
            },
            UpdateConfigEvent::Restore {key, message} => {
                self.temp_texts.remove(&key);
                (false, Some(*message))
            },
            UpdateConfigEvent::SaveRestorePoint => {
                if !self.config_manager.save_restore_point() {
                    return Task::done(
                        KeyboardNotification::Error(Arc::new(anyhow::anyhow!(
                            "failed to save the restore point"
                        )))
                        .into(),
                    );
                }
                return Task::done(
                    KeyboardNotification::Info("Restore point saved".to_string()).into(),
                );
            },
            UpdateConfigEvent::RestoreAll(messages) => {
                return Task::batch(messages.into_iter().map(Task::done));
            },
            UpdateConfigEvent::ChangeTempText {key, init_value, value} => {
                tracing::debug!("Update temp_text[{key}] to {value}, init value[{init_value}]");
                self.temp_texts.insert(key, (init_value, value));
                (false, None)
            },
            UpdateConfigEvent::SubmitTempText {key, init_value, producer} => {
                let mut message = None;
                if let Some((last_init_value, value)) = self.temp_texts.remove(&key) {
                    // update only if init_value is the same
                    if last_init_value == init_value && value != init_value {
                        message = Some(producer(value))
                    }
                };
                (false, message)
            },
        );
        if updated {
            self.config_manager.try_write();
        }
        message.map(Task::done).unwrap_or_else(Message::nothing)
    }
}

fn sane_volume(v: f32) -> f32 {
    if v.is_finite() { v.clamp(0., 1.) } else { 0.5 }
}

fn default_should_display(_state: &dyn StateExtractor) -> bool {
    true
}

fn preferred_output_name_field() -> Field {
    Field {
        name: "Preferred Output",
        id: "preferred_output_name",
        typ: DynamicEnumDesc::<String> {
            variants_and_selected: |state| {
                let outputs = state.outputs();
                let mut variants = Vec::with_capacity(outputs.len());
                let mut selected = None;
                let preferred_output_name = state.config().preferred_output_name();

                for (name, description) in outputs {
                    if preferred_output_name.filter(|n| **n == name).is_some() {
                        selected = Some(output_name(name.clone(), description.clone()));
                    }
                    variants.push(output_name(name, description));
                }
                if selected.is_none()
                    && let Some(name) = preferred_output_name
                {
                    selected = Some(ValueAndDescription {
                        value: name.clone(),
                        desc: format!("{name} (Not Connected)"),
                    })
                }
                (variants, selected)
            },
            is_enabled: |_| true,
            on_selected: |_, d| Message::from(UpdateConfigEvent::PreferredOutputName(d.value)),
        }
        .into(),
        should_display: default_should_display,
    }
}

fn preferred_output_name_custom_field() -> Field {
    Field {
        name: "Preferred Output(Custom)",
        id: "preferred_output_name",
        typ: TextDesc {
            placeholder: |_, _| Some("The name of the preferred output, like: DP-1".to_string()),
            is_enabled: |_| true,
            init_value: |state| state.config().preferred_output_name().cloned(),
            submit_message: |s| UpdateConfigEvent::PreferredOutputName(s).into(),
        }
        .into(),
        should_display: default_should_display,
    }
}

fn output_name(name: String, description: String) -> ValueAndDescription<String> {
    if description.is_empty() {
        ValueAndDescription {
            value: name.clone(),
            desc: name,
        }
    } else {
        ValueAndDescription {
            value: name.clone(),
            desc: format!("{name} ({description})"),
        }
    }
}

#[derive(Clone, Debug)]
pub enum UpdateConfigEvent {
    LandscapeWidth(KLength),
    PortraitWidth(KLength),
    Placement(Placement),
    IndicatorDisplay(IndicatorDisplay),
    Theme(String),
    KeyBorderRadius(f32),
    KeyAnimation(bool),
    KeyPreview(bool),
    KeySound(KeySound),
    KeySoundVolume(f32),
    KeyHeightScale(f32),
    KeySpacingScale(f32),
    DarkTheme(String),
    LightTheme(String),
    PreferredOutputName(String),
    /// Drop the unsubmitted slider value of `key` and apply `message`.
    Restore {
        key: String,
        message: Box<Message>,
    },
    SaveRestorePoint,
    RestoreAll(Vec<Message>),
    ChangeTempText {
        key: String,
        init_value: String,
        value: String,
    },
    SubmitTempText {
        key: String,
        init_value: String,
        producer: fn(String) -> Message,
    },
    ManualMode(bool),
    IgnoreFcitxShow(bool),
    AutoShowOnFocus(bool),
    AutoHideOnBlur(bool),
    AutoHideDelay(Duration),
    AutoShowTouchOnly(bool),
    QuickActionBarState(QuickActionBarState),
    CustomActions(Arc<Vec<String>>),
}

impl From<UpdateConfigEvent> for Message {
    fn from(value: UpdateConfigEvent) -> Self {
        Self::UpdateConfigEvent(value)
    }
}
