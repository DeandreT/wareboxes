use leptos::prelude::*;
use lucide_leptos::{Building2, RefreshCw, Save, X};
use wareboxes_api_contract::v1::{
    ConfigureFacilityShippingOriginRequest, ConfigureFacilityShippingOriginResponse, Revision,
};

use crate::api;
use crate::toast::use_toast_bus;

const MAX_NAME_LENGTH: usize = 200;
const MAX_COMPANY_LENGTH: usize = 200;
const MAX_ADDRESS_LINE_LENGTH: usize = 200;
const MAX_CITY_LENGTH: usize = 100;
const MAX_STATE_LENGTH: usize = 100;
const MAX_POSTAL_CODE_LENGTH: usize = 32;
const MAX_COUNTRY_LENGTH: usize = 100;
const MAX_PHONE_LENGTH: usize = 64;
const MAX_EMAIL_LENGTH: usize = 254;

#[derive(Clone, Debug, PartialEq, Eq)]
struct OriginDraft {
    name: String,
    company: String,
    line1: String,
    line2: String,
    city: String,
    state: String,
    postal_code: String,
    country: String,
    phone: String,
    email: String,
}

impl Default for OriginDraft {
    fn default() -> Self {
        Self {
            name: String::new(),
            company: String::new(),
            line1: String::new(),
            line2: String::new(),
            city: String::new(),
            state: String::new(),
            postal_code: String::new(),
            country: "US".to_owned(),
            phone: String::new(),
            email: String::new(),
        }
    }
}

impl OriginDraft {
    fn request(&self, revision: i64) -> Result<ConfigureFacilityShippingOriginRequest, String> {
        let name = optional_text(&self.name, "Origin name", MAX_NAME_LENGTH)?;
        let company = optional_text(&self.company, "Company", MAX_COMPANY_LENGTH)?;
        if name.is_none() && company.is_none() {
            return Err("Enter an origin name or company.".to_owned());
        }
        let line1 = required_text(&self.line1, "Address line 1", MAX_ADDRESS_LINE_LENGTH)?;
        let line2 = optional_text(&self.line2, "Address line 2", MAX_ADDRESS_LINE_LENGTH)?;
        let city = required_text(&self.city, "City", MAX_CITY_LENGTH)?;
        let state = optional_text(&self.state, "State / region", MAX_STATE_LENGTH)?;
        let postal_code = required_text(&self.postal_code, "Postal code", MAX_POSTAL_CODE_LENGTH)?;
        let country = required_text(&self.country, "Country", MAX_COUNTRY_LENGTH)?;
        let phone = optional_text(&self.phone, "Phone", MAX_PHONE_LENGTH)?;
        let email = optional_text(&self.email, "Email", MAX_EMAIL_LENGTH)?;
        let expected_revision = Revision::new(revision)
            .map_err(|_| "The facility revision is invalid. Refresh shipping work.".to_owned())?;

        Ok(ConfigureFacilityShippingOriginRequest {
            expected_revision,
            name,
            company,
            line1,
            line2,
            city,
            state,
            postal_code,
            country,
            phone,
            email,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct OriginAttempt {
    facility_id: i64,
    request: ConfigureFacilityShippingOriginRequest,
    idempotency_key: String,
}

#[component]
pub fn FacilityShippingOriginDialog(
    facility_id: Signal<i64>,
    facility_name: Signal<String>,
    current_revision: Signal<i64>,
    on_close: Callback<()>,
    on_configured: Callback<ConfigureFacilityShippingOriginResponse>,
) -> impl IntoView {
    let draft = RwSignal::new(OriginDraft::default());
    let pending = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let retry_attempt = RwSignal::new(None::<OriginAttempt>);
    let toasts = use_toast_bus();
    let locked = move || pending.get() || retry_attempt.get().is_some();

    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if pending.get_untracked() {
            return;
        }
        let attempt = if let Some(attempt) = retry_attempt.get_untracked() {
            attempt
        } else {
            let request = match draft
                .get_untracked()
                .request(current_revision.get_untracked())
            {
                Ok(request) => request,
                Err(message) => {
                    error.set(Some(message));
                    return;
                }
            };
            OriginAttempt {
                facility_id: facility_id.get_untracked(),
                request,
                idempotency_key: api::new_idempotency_key(),
            }
        };

        retry_attempt.set(Some(attempt.clone()));
        pending.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            match api::configure_facility_shipping_origin(
                attempt.facility_id,
                &attempt.request,
                &attempt.idempotency_key,
            )
            .await
            {
                Ok(result) => {
                    pending.set(false);
                    retry_attempt.set(None);
                    toasts.success(format!(
                        "Shipping origin configured for {} at revision {}.",
                        facility_name.get_untracked(),
                        result.revision.get()
                    ));
                    on_configured.run(result);
                    on_close.run(());
                }
                Err(api_error) => {
                    pending.set(false);
                    if !retain_attempt(api_error.ambiguous_outcome) {
                        retry_attempt.set(None);
                    }
                    let message = if api_error.ambiguous_outcome {
                        format!(
                            "{} The result is unknown; retry the retained request.",
                            api_error.message
                        )
                    } else {
                        api_error.message.clone()
                    };
                    error.set(Some(message));
                    toasts.error(api_error.message);
                }
            }
        });
    };

    view! {
        <div class="facility-origin-backdrop">
            <section
                class="facility-origin-dialog"
                role="alertdialog"
                aria-modal="true"
                aria-labelledby="facility-origin-title"
                aria-describedby="facility-origin-description"
            >
                <header class="facility-origin-heading">
                    <span class="facility-origin-icon" aria-hidden="true">
                        <Building2 size=18/>
                    </span>
                    <div>
                        <h2 id="facility-origin-title">"Configure shipping origin"</h2>
                        <span>{move || format!(
                            "{} / Facility #{} / Revision {}",
                            facility_name.get(),
                            facility_id.get(),
                            current_revision.get()
                        )}</span>
                    </div>
                    <button
                        type="button"
                        class="facility-origin-close"
                        title="Close shipping origin configuration"
                        aria-label="Close shipping origin configuration"
                        disabled=locked
                        on:click=move |_| on_close.run(())
                    >
                        <X size=16/>
                    </button>
                </header>
                <p id="facility-origin-description" class="sr-only">
                    "Configure the carrier-facing address used as this facility's shipment origin."
                </p>
                <form class="facility-origin-form" on:submit=submit>
                    <div class="facility-origin-grid">
                        <label>
                            <span>"Origin name"</span>
                            <input
                                autofocus=true
                                autocomplete="name"
                                maxlength=MAX_NAME_LENGTH
                                disabled=locked
                                prop:value=move || draft.get().name
                                on:input=move |event| {
                                    draft.update(|draft| draft.name = event_target_value(&event));
                                    error.set(None);
                                }
                            />
                        </label>
                        <label>
                            <span>"Company"</span>
                            <input
                                autocomplete="organization"
                                maxlength=MAX_COMPANY_LENGTH
                                disabled=locked
                                prop:value=move || draft.get().company
                                on:input=move |event| {
                                    draft.update(|draft| draft.company = event_target_value(&event));
                                    error.set(None);
                                }
                            />
                        </label>
                        <label class="wide">
                            <span>"Address line 1"</span>
                            <input
                                required=true
                                autocomplete="address-line1"
                                maxlength=MAX_ADDRESS_LINE_LENGTH
                                disabled=locked
                                prop:value=move || draft.get().line1
                                on:input=move |event| {
                                    draft.update(|draft| draft.line1 = event_target_value(&event));
                                    error.set(None);
                                }
                            />
                        </label>
                        <label class="wide">
                            <span>"Address line 2"</span>
                            <input
                                autocomplete="address-line2"
                                maxlength=MAX_ADDRESS_LINE_LENGTH
                                disabled=locked
                                prop:value=move || draft.get().line2
                                on:input=move |event| {
                                    draft.update(|draft| draft.line2 = event_target_value(&event));
                                    error.set(None);
                                }
                            />
                        </label>
                        <label>
                            <span>"City"</span>
                            <input
                                required=true
                                autocomplete="address-level2"
                                maxlength=MAX_CITY_LENGTH
                                disabled=locked
                                prop:value=move || draft.get().city
                                on:input=move |event| {
                                    draft.update(|draft| draft.city = event_target_value(&event));
                                    error.set(None);
                                }
                            />
                        </label>
                        <label>
                            <span>"Postal code"</span>
                            <input
                                required=true
                                autocomplete="postal-code"
                                maxlength=MAX_POSTAL_CODE_LENGTH
                                disabled=locked
                                prop:value=move || draft.get().postal_code
                                on:input=move |event| {
                                    draft.update(|draft| draft.postal_code = event_target_value(&event));
                                    error.set(None);
                                }
                            />
                        </label>
                        <label>
                            <span>"State / region"</span>
                            <input
                                autocomplete="address-level1"
                                maxlength=MAX_STATE_LENGTH
                                disabled=locked
                                prop:value=move || draft.get().state
                                on:input=move |event| {
                                    draft.update(|draft| draft.state = event_target_value(&event));
                                    error.set(None);
                                }
                            />
                        </label>
                        <label>
                            <span>"Country"</span>
                            <input
                                required=true
                                autocomplete="country"
                                maxlength=MAX_COUNTRY_LENGTH
                                disabled=locked
                                prop:value=move || draft.get().country
                                on:input=move |event| {
                                    draft.update(|draft| draft.country = event_target_value(&event));
                                    error.set(None);
                                }
                            />
                        </label>
                        <label>
                            <span>"Phone"</span>
                            <input
                                type="tel"
                                autocomplete="tel"
                                maxlength=MAX_PHONE_LENGTH
                                disabled=locked
                                prop:value=move || draft.get().phone
                                on:input=move |event| {
                                    draft.update(|draft| draft.phone = event_target_value(&event));
                                    error.set(None);
                                }
                            />
                        </label>
                        <label>
                            <span>"Email"</span>
                            <input
                                type="email"
                                autocomplete="email"
                                maxlength=MAX_EMAIL_LENGTH
                                disabled=locked
                                prop:value=move || draft.get().email
                                on:input=move |event| {
                                    draft.update(|draft| draft.email = event_target_value(&event));
                                    error.set(None);
                                }
                            />
                        </label>
                    </div>
                    <Show when=move || error.get().is_some()>
                        <p class="inline-command-error facility-origin-error" role="alert">
                            {move || error.get().unwrap_or_default()}
                        </p>
                    </Show>
                    <Show when=move || retry_attempt.get().is_some() && !pending.get()>
                        <p class="facility-origin-retry" role="status">
                            "The original request is retained. Retry to recover its result."
                        </p>
                    </Show>
                    <div class="form-actions">
                        <button
                            type="button"
                            class="button secondary-action"
                            disabled=locked
                            on:click=move |_| on_close.run(())
                        >
                            "Cancel"
                        </button>
                        <button
                            type="submit"
                            class="button primary-action"
                            disabled=move || pending.get()
                        >
                            {move || if retry_attempt.get().is_some() {
                                view! { <RefreshCw size=15/> }.into_any()
                            } else {
                                view! { <Save size=15/> }.into_any()
                            }}
                            {move || if pending.get() {
                                "Saving"
                            } else if retry_attempt.get().is_some() {
                                "Retry save"
                            } else {
                                "Save origin"
                            }}
                        </button>
                    </div>
                </form>
            </section>
        </div>
    }
}

fn required_text(value: &str, label: &str, maximum: usize) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("{label} is required."));
    }
    validate_text(value, label, maximum)?;
    Ok(value.to_owned())
}

fn optional_text(value: &str, label: &str, maximum: usize) -> Result<Option<String>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    validate_text(value, label, maximum)?;
    Ok(Some(value.to_owned()))
}

fn validate_text(value: &str, label: &str, maximum: usize) -> Result<(), String> {
    if value.chars().any(char::is_control) {
        return Err(format!("{label} contains an unsupported character."));
    }
    if value.chars().count() > maximum {
        return Err(format!("{label} cannot exceed {maximum} characters."));
    }
    Ok(())
}

const fn retain_attempt(ambiguous_outcome: bool) -> bool {
    ambiguous_outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete_draft() -> OriginDraft {
        OriginDraft {
            name: "  West shipping office  ".into(),
            company: String::new(),
            line1: " 100 Distribution Way ".into(),
            line2: String::new(),
            city: " Reno ".into(),
            state: String::new(),
            postal_code: " 89502 ".into(),
            country: " US ".into(),
            phone: String::new(),
            email: String::new(),
        }
    }

    #[test]
    fn draft_builds_a_trimmed_complete_request_with_optional_region() {
        let request = complete_draft().request(3).unwrap();
        assert_eq!(request.expected_revision.get(), 3);
        assert_eq!(request.name.as_deref(), Some("West shipping office"));
        assert_eq!(request.company, None);
        assert_eq!(request.line1, "100 Distribution Way");
        assert_eq!(request.state, None);
        assert_eq!(request.country, "US");
    }

    #[test]
    fn draft_requires_a_name_and_complete_address() {
        let mut draft = complete_draft();
        draft.name.clear();
        assert_eq!(
            draft.request(1).unwrap_err(),
            "Enter an origin name or company."
        );

        draft.company = "Wareboxes Fulfillment".into();
        draft.postal_code.clear();
        assert_eq!(draft.request(1).unwrap_err(), "Postal code is required.");
    }

    #[test]
    fn draft_enforces_bounds_and_positive_revision() {
        let mut draft = complete_draft();
        draft.email = "x".repeat(MAX_EMAIL_LENGTH + 1);
        assert_eq!(
            draft.request(1).unwrap_err(),
            "Email cannot exceed 254 characters."
        );
        assert_eq!(
            complete_draft().request(0).unwrap_err(),
            "The facility revision is invalid. Refresh shipping work."
        );
    }

    #[test]
    fn only_ambiguous_failures_retain_the_exact_attempt() {
        assert!(retain_attempt(true));
        assert!(!retain_attempt(false));
    }

    #[cfg(feature = "ssr")]
    #[test]
    fn dialog_renders_a_compact_accessible_form_surface() {
        use crate::toast::ToastProvider;

        let html = Owner::new().with(|| {
            let facility_id = RwSignal::new(17_i64);
            let facility_name = RwSignal::new("Reno DC".to_owned());
            let revision = RwSignal::new(4_i64);
            view! {
                <ToastProvider>
                    <FacilityShippingOriginDialog
                        facility_id=Signal::derive(move || facility_id.get())
                        facility_name=Signal::derive(move || facility_name.get())
                        current_revision=Signal::derive(move || revision.get())
                        on_close=Callback::new(|()| {})
                        on_configured=Callback::new(
                            |_: ConfigureFacilityShippingOriginResponse| {}
                        )
                    />
                </ToastProvider>
            }
            .to_html()
        });

        assert!(html.contains("role=\"alertdialog\""));
        assert!(html.contains("aria-modal=\"true\""));
        assert!(html.contains("Reno DC / Facility #17 / Revision 4"));
        assert!(html.contains("autocomplete=\"address-line1\""));
        assert_eq!(html.matches("<input").count(), 10);
    }
}
