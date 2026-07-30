use leptos::prelude::*;
use wareboxes_core::dto::{AddEmployee, EmployeeIdRequest, EmployeeUpdate};
use wareboxes_core::models::{Employee, Facility};

use super::{
    facility_names, optional_text, status_class, DeletedToggle, FacilityChecks, InlineCommandError,
    WorkbenchError, WorkbenchLoading,
};
use crate::api;
use crate::components::SearchField;
use crate::sorting::{SortDirection, SortSpec, SortableHeader};
use crate::toast::use_toast_bus;

#[derive(Clone, Copy, PartialEq, Eq)]
enum EmployeeSort {
    Name,
    Title,
    Type,
    Facilities,
    Status,
}

#[derive(Clone, Copy)]
struct EmployeeDraft {
    first_name: RwSignal<String>,
    last_name: RwSignal<String>,
    title: RwSignal<String>,
    employee_type: RwSignal<String>,
    email: RwSignal<String>,
    phone: RwSignal<String>,
    facility_ids: RwSignal<Vec<i64>>,
}

impl EmployeeDraft {
    fn new() -> Self {
        Self {
            first_name: RwSignal::new(String::new()),
            last_name: RwSignal::new(String::new()),
            title: RwSignal::new(String::new()),
            employee_type: RwSignal::new(String::new()),
            email: RwSignal::new(String::new()),
            phone: RwSignal::new(String::new()),
            facility_ids: RwSignal::new(Vec::new()),
        }
    }

    fn load(&self, employee: &Employee) {
        self.first_name.set(employee.first_name.clone());
        self.last_name.set(employee.last_name.clone());
        self.title.set(employee.title.clone());
        self.employee_type.set(employee.r#type.clone());
        self.email.set(employee.email.clone().unwrap_or_default());
        self.phone.set(employee.phone.clone().unwrap_or_default());
        self.facility_ids.set(employee.facility_ids.clone());
    }

    fn clear(&self) {
        self.first_name.set(String::new());
        self.last_name.set(String::new());
        self.title.set(String::new());
        self.employee_type.set(String::new());
        self.email.set(String::new());
        self.phone.set(String::new());
        self.facility_ids.set(Vec::new());
    }

    fn valid(&self) -> bool {
        !self.first_name.get_untracked().trim().is_empty()
            && !self.last_name.get_untracked().trim().is_empty()
            && !self.title.get_untracked().trim().is_empty()
            && !self.employee_type.get_untracked().trim().is_empty()
            && !self.facility_ids.get_untracked().is_empty()
    }
}

#[component]
pub fn EmployeesWorkbench(on_unauthorized: Callback<()>) -> impl IntoView {
    let employees = RwSignal::new(Vec::<Employee>::new());
    let facilities = RwSignal::new(Vec::<Facility>::new());
    let loading = RwSignal::new(true);
    let load_error = RwSignal::new(None::<String>);
    let command_error = RwSignal::new(None::<String>);
    let pending = RwSignal::new(false);
    let show_deleted = RwSignal::new(false);
    let filter = RwSignal::new(String::new());
    let selected_id = RwSignal::new(None::<i64>);
    let create_draft = EmployeeDraft::new();
    let edit_draft = EmployeeDraft::new();
    let sort = RwSignal::new(SortSpec {
        key: EmployeeSort::Name,
        direction: SortDirection::Ascending,
    });
    let toasts = use_toast_bus();

    let refresh = Callback::new(move |_| {
        refresh_employees(
            show_deleted.get_untracked(),
            employees,
            facilities,
            loading,
            load_error,
            on_unauthorized,
        );
    });
    Effect::new(move || {
        let _ = show_deleted.get();
        refresh.run(());
    });

    let create_draft_submit = create_draft;
    let create = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if pending.get_untracked() || !create_draft_submit.valid() {
            command_error.set(Some(
                "Enter a name, title, employee type, and at least one facility.".to_owned(),
            ));
            return;
        }
        let request = AddEmployee {
            first_name: create_draft_submit
                .first_name
                .get_untracked()
                .trim()
                .to_owned(),
            last_name: create_draft_submit
                .last_name
                .get_untracked()
                .trim()
                .to_owned(),
            title: create_draft_submit.title.get_untracked().trim().to_owned(),
            r#type: create_draft_submit
                .employee_type
                .get_untracked()
                .trim()
                .to_owned(),
            email: optional_text(&create_draft_submit.email.get_untracked()),
            phone: optional_text(&create_draft_submit.phone.get_untracked()),
            hired: None,
            facility_ids: create_draft_submit.facility_ids.get_untracked(),
        };
        let name = format!("{} {}", request.first_name, request.last_name);
        let draft = create_draft_submit;
        pending.set(true);
        command_error.set(None);
        leptos::task::spawn_local(async move {
            match api::internal_post::<_, i64>("/api/employees/add", &request).await {
                Ok(_) => {
                    draft.clear();
                    toasts.success(format!("{name} added to the workforce."));
                    refresh.run(());
                }
                Err(error) if error.unauthorized => on_unauthorized.run(()),
                Err(error) => {
                    toasts.error(error.message.clone());
                    command_error.set(Some(error.message));
                }
            }
            pending.set(false);
        });
    };

    let edit_draft_submit = edit_draft;
    let save = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let Some(employee_id) = selected_id.get_untracked() else {
            return;
        };
        if pending.get_untracked() || !edit_draft_submit.valid() {
            command_error.set(Some(
                "Enter a name, title, employee type, and at least one facility.".to_owned(),
            ));
            return;
        }
        let request = EmployeeUpdate {
            employee_id,
            first_name: optional_text(&edit_draft_submit.first_name.get_untracked()),
            last_name: optional_text(&edit_draft_submit.last_name.get_untracked()),
            title: optional_text(&edit_draft_submit.title.get_untracked()),
            r#type: optional_text(&edit_draft_submit.employee_type.get_untracked()),
            email: optional_text(&edit_draft_submit.email.get_untracked()),
            phone: optional_text(&edit_draft_submit.phone.get_untracked()),
            terminated: None,
            facility_ids: Some(edit_draft_submit.facility_ids.get_untracked()),
        };
        let name = format!(
            "{} {}",
            edit_draft_submit.first_name.get_untracked().trim(),
            edit_draft_submit.last_name.get_untracked().trim()
        );
        pending.set(true);
        command_error.set(None);
        leptos::task::spawn_local(async move {
            match api::internal_post::<_, bool>("/api/employees/update", &request).await {
                Ok(true) => {
                    toasts.success(format!("{name} updated."));
                    refresh.run(());
                }
                Ok(false) => {
                    let message = "The selected employee no longer exists.".to_owned();
                    toasts.error(message.clone());
                    command_error.set(Some(message));
                }
                Err(error) if error.unauthorized => on_unauthorized.run(()),
                Err(error) => {
                    toasts.error(error.message.clone());
                    command_error.set(Some(error.message));
                }
            }
            pending.set(false);
        });
    };

    let set_active = move |employee: Employee, active: bool| {
        if pending.get_untracked() || !employee.can_manage {
            return;
        }
        let path = if active {
            "/api/employees/restore"
        } else {
            "/api/employees/delete"
        };
        let name = employee_name(&employee);
        pending.set(true);
        command_error.set(None);
        leptos::task::spawn_local(async move {
            match api::internal_post::<_, bool>(
                path,
                &EmployeeIdRequest {
                    employee_id: employee.id,
                },
            )
            .await
            {
                Ok(true) => {
                    if !active && selected_id.get_untracked() == Some(employee.id) {
                        selected_id.set(None);
                    }
                    let state = if active { "reactivated" } else { "deactivated" };
                    toasts.success(format!("{name} {state}."));
                    refresh.run(());
                }
                Ok(false) => {
                    let message = "The selected employee is no longer manageable.".to_owned();
                    toasts.error(message.clone());
                    command_error.set(Some(message));
                }
                Err(error) if error.unauthorized => on_unauthorized.run(()),
                Err(error) => {
                    toasts.error(error.message.clone());
                    command_error.set(Some(error.message));
                }
            }
            pending.set(false);
        });
    };
    let row_context = EmployeeRowsContext {
        selected_id,
        edit_draft,
        command_error,
        pending,
        set_active: Callback::new(move |(employee, active)| set_active(employee, active)),
    };

    view! {
        <section class="admin-workbench">
            <details class="admin-create">
                <summary>"Add employee"</summary>
                <form class="admin-create-form" on:submit=create>
                    <EmployeeFields draft=create_draft/>
                    <FacilityChecks
                        facilities=Signal::derive(move || facilities.get())
                        selected=create_draft.facility_ids
                        legend="Facility assignments"
                    />
                    <div class="admin-form-actions">
                        <button class="button primary-action compact" type="submit" disabled=move || pending.get()>
                            "Add employee"
                        </button>
                    </div>
                </form>
            </details>

            <div class="admin-toolbar">
                <SearchField label="Filter employees".to_owned() placeholder="Filter employees" value=filter/>
                <div class="admin-toolbar-actions">
                    <DeletedToggle show_deleted/>
                    <button class="button secondary-action compact" type="button" on:click=move |_| refresh.run(())>"Refresh"</button>
                </div>
            </div>
            <InlineCommandError message=command_error.read_only()/>

            {move || {
                if loading.get() {
                    view! { <WorkbenchLoading label="employees"/> }.into_any()
                } else if let Some(message) = load_error.get() {
                    view! { <WorkbenchError message retry=refresh/> }.into_any()
                } else {
                    view! {
                        <div class="admin-split">
                            <section class="admin-list">
                                <div class="table-scroll">
                                    <table class="data-table admin-table">
                                        <caption class="sr-only">"Employees in this organization"</caption>
                                        <thead>
                                            <tr>
                                                <SortableHeader label="Employee" active=move || sort.get().key == EmployeeSort::Name direction=move || sort.get().direction on_sort=Callback::new(move |_| SortSpec::select(sort, EmployeeSort::Name))/>
                                                <SortableHeader label="Title" active=move || sort.get().key == EmployeeSort::Title direction=move || sort.get().direction on_sort=Callback::new(move |_| SortSpec::select(sort, EmployeeSort::Title))/>
                                                <SortableHeader label="Type" active=move || sort.get().key == EmployeeSort::Type direction=move || sort.get().direction on_sort=Callback::new(move |_| SortSpec::select(sort, EmployeeSort::Type))/>
                                                <SortableHeader label="Facilities" active=move || sort.get().key == EmployeeSort::Facilities direction=move || sort.get().direction on_sort=Callback::new(move |_| SortSpec::select(sort, EmployeeSort::Facilities))/>
                                                <SortableHeader label="Status" active=move || sort.get().key == EmployeeSort::Status direction=move || sort.get().direction on_sort=Callback::new(move |_| SortSpec::select(sort, EmployeeSort::Status))/>
                                                <th scope="col" class="action-column">"Actions"</th>
                                            </tr>
                                        </thead>
                                        <tbody>
                                            {move || employee_rows(
                                                employees.get(),
                                                facilities.get(),
                                                filter.get(),
                                                sort.get(),
                                                row_context,
                                            )}
                                        </tbody>
                                    </table>
                                </div>
                            </section>
                            <section class="admin-editor" aria-label="Employee editor">
                                {move || {
                                    selected_id.get().map_or_else(
                                        || view! { <div class="admin-editor-placeholder">"Select an employee to edit contact and facility assignments."</div> }.into_any(),
                                        |id| {
                                            view! {
                                                <div class="admin-editor-heading"><h2>"Edit employee"</h2><span>{format!("#{id}")}</span></div>
                                                <form class="admin-form" on:submit=save>
                                                    <EmployeeFields draft=edit_draft/>
                                                    <FacilityChecks
                                                        facilities=Signal::derive(move || facilities.get())
                                                        selected=edit_draft.facility_ids
                                                        legend="Facility assignments"
                                                    />
                                                    <div class="admin-form-actions">
                                                        <button type="button" class="button quiet-action compact" on:click=move |_| selected_id.set(None)>"Cancel"</button>
                                                        <button type="submit" class="button primary-action compact" disabled=move || pending.get()>"Save"</button>
                                                    </div>
                                                </form>
                                            }
                                                .into_any()
                                        },
                                    )
                                }}
                            </section>
                        </div>
                    }
                        .into_any()
                }
            }}
        </section>
    }
}

#[component]
fn EmployeeFields(draft: EmployeeDraft) -> impl IntoView {
    view! {
        <div class="admin-form-grid">
            <label>
                <span>"First name"</span>
                <input type="text" required prop:value=move || draft.first_name.get() on:input=move |event| draft.first_name.set(event_target_value(&event))/>
            </label>
            <label>
                <span>"Last name"</span>
                <input type="text" required prop:value=move || draft.last_name.get() on:input=move |event| draft.last_name.set(event_target_value(&event))/>
            </label>
            <label>
                <span>"Title"</span>
                <input type="text" required prop:value=move || draft.title.get() on:input=move |event| draft.title.set(event_target_value(&event))/>
            </label>
            <label>
                <span>"Employee type"</span>
                <input type="text" required placeholder="Operator, lead, manager" prop:value=move || draft.employee_type.get() on:input=move |event| draft.employee_type.set(event_target_value(&event))/>
            </label>
            <label>
                <span>"Email"</span>
                <input type="email" prop:value=move || draft.email.get() on:input=move |event| draft.email.set(event_target_value(&event))/>
            </label>
            <label>
                <span>"Phone"</span>
                <input type="tel" prop:value=move || draft.phone.get() on:input=move |event| draft.phone.set(event_target_value(&event))/>
            </label>
        </div>
    }
}

#[derive(Clone, Copy)]
struct EmployeeRowsContext {
    selected_id: RwSignal<Option<i64>>,
    edit_draft: EmployeeDraft,
    command_error: RwSignal<Option<String>>,
    pending: RwSignal<bool>,
    set_active: Callback<(Employee, bool)>,
}

fn employee_rows(
    mut rows: Vec<Employee>,
    facilities: Vec<Facility>,
    filter: String,
    spec: SortSpec<EmployeeSort>,
    context: EmployeeRowsContext,
) -> AnyView {
    let query = filter.trim().to_ascii_lowercase();
    rows.retain(|employee| {
        query.is_empty()
            || employee_name(employee)
                .to_ascii_lowercase()
                .contains(&query)
            || employee.title.to_ascii_lowercase().contains(&query)
            || employee.r#type.to_ascii_lowercase().contains(&query)
            || employee
                .email
                .as_deref()
                .unwrap_or_default()
                .to_ascii_lowercase()
                .contains(&query)
    });
    sort_employees(&mut rows, spec);
    if rows.is_empty() {
        return view! { <tr><td class="table-empty-row" colspan="6">"No matching employees."</td></tr> }
            .into_any();
    }
    rows.into_iter()
        .map(|employee| {
            let edit_employee = employee.clone();
            let active_employee = employee.clone();
            let inactive = employee.deleted.is_some();
            let can_manage = employee.can_manage;
            let assignments = facility_names(&employee.facility_ids, &facilities);
            let assignments_title = assignments.clone();
            view! {
                <tr class:selected-row=context.selected_id.get() == Some(employee.id)>
                    <td>
                        <div class="cell-stack">
                            <strong>{employee_name(&employee)}</strong>
                            <small>{employee.email.clone().unwrap_or_else(|| format!("Employee #{}", employee.id))}</small>
                        </div>
                    </td>
                    <td>{employee.title.clone()}</td>
                    <td>{employee.r#type.clone()}</td>
                    <td title=assignments_title>{assignments}</td>
                    <td><span class=status_class(inactive)>{if inactive { "Inactive" } else { "Active" }}</span></td>
                    <td class="action-column">
                        <div class="admin-row-actions">
                            <button
                                type="button"
                                class="table-action"
                                disabled=!can_manage
                                on:click=move |_| {
                                    context.selected_id.set(Some(edit_employee.id));
                                    context.edit_draft.load(&edit_employee);
                                    context.command_error.set(None);
                                }
                            >
                                "Edit"
                            </button>
                            <button
                                type="button"
                                class=if inactive { "table-action" } else { "table-action danger" }
                                disabled=move || context.pending.get() || !can_manage
                                on:click=move |_| context.set_active.run((active_employee.clone(), inactive))
                            >
                                {if inactive { "Reactivate" } else { "Deactivate" }}
                            </button>
                        </div>
                    </td>
                </tr>
            }
        })
        .collect_view()
        .into_any()
}

fn employee_name(employee: &Employee) -> String {
    format!("{} {}", employee.first_name, employee.last_name)
        .trim()
        .to_owned()
}

fn refresh_employees(
    show_deleted: bool,
    employees: RwSignal<Vec<Employee>>,
    facilities: RwSignal<Vec<Facility>>,
    loading: RwSignal<bool>,
    load_error: RwSignal<Option<String>>,
    on_unauthorized: Callback<()>,
) {
    loading.set(true);
    load_error.set(None);
    leptos::task::spawn_local(async move {
        let result = async {
            let employees = api::internal_get::<Vec<Employee>>(&format!(
                "/api/employees?show_deleted={show_deleted}"
            ))
            .await?;
            let facilities =
                api::internal_get::<Vec<Facility>>("/api/facilities?show_deleted=true").await?;
            Ok::<_, api::ApiError>((employees, facilities))
        }
        .await;
        match result {
            Ok((new_employees, new_facilities)) => {
                employees.set(new_employees);
                facilities.set(new_facilities);
            }
            Err(error) if error.unauthorized => on_unauthorized.run(()),
            Err(error) => load_error.set(Some(error.message)),
        }
        loading.set(false);
    });
}

fn sort_employees(rows: &mut [Employee], spec: SortSpec<EmployeeSort>) {
    rows.sort_by(|left, right| {
        let ordering = match spec.key {
            EmployeeSort::Name => employee_name(left)
                .to_ascii_lowercase()
                .cmp(&employee_name(right).to_ascii_lowercase()),
            EmployeeSort::Title => left
                .title
                .to_ascii_lowercase()
                .cmp(&right.title.to_ascii_lowercase()),
            EmployeeSort::Type => left
                .r#type
                .to_ascii_lowercase()
                .cmp(&right.r#type.to_ascii_lowercase()),
            EmployeeSort::Facilities => left.facility_ids.len().cmp(&right.facility_ids.len()),
            EmployeeSort::Status => left.deleted.is_some().cmp(&right.deleted.is_some()),
        }
        .then_with(|| left.id.cmp(&right.id));
        if spec.direction == SortDirection::Ascending {
            ordering
        } else {
            ordering.reverse()
        }
    });
}
