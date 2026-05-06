use super::{
    checkable_state_expression, fill_expression, focus_expression, key_text, scroll_delta,
    scroll_into_view_expression, select_expression, snapshot_expression,
};

#[test]
fn fill_expression_supports_label_controls() {
    let expression = fill_expression(10.0, 20.0, "pufferfish").unwrap();
    assert!(expression.contains("label.control"));
    assert!(expression.contains("label.querySelector(editableSelector)"));
}

#[test]
fn fill_expression_uses_native_value_setter() {
    let expression = fill_expression(10.0, 20.0, "pufferfish").unwrap();
    assert!(expression.contains("Object.getOwnPropertyDescriptor(prototype, 'value')"));
    assert!(expression.contains("descriptor.set.call(target"));
}

#[test]
fn snapshot_expression_avoids_ref_for_form_containers() {
    let expression = snapshot_expression();
    assert!(expression.contains("if (tag === 'form') return false;"));
    assert!(expression.contains("querySelectorAll(interactiveSelector)"));
}

#[test]
fn focus_expression_targets_focusable_elements() {
    let expression = focus_expression(10.0, 20.0);
    assert!(expression.contains("target.focus"));
    assert!(expression.contains("Target is not focusable"));
}

#[test]
fn scroll_helpers_cover_alias_behaviour() {
    assert_eq!(scroll_delta("down", 480).unwrap(), (0.0, 480.0));
    assert!(scroll_delta("diagonal", 480).is_err());
    assert_eq!(key_text("A").as_deref(), Some("A"));
    assert_eq!(key_text("Enter"), None);
    let expression = scroll_into_view_expression(10.0, 20.0);
    assert!(expression.contains("scrollIntoView"));
    assert!(expression.contains("behavior: 'instant'"));
}

#[test]
fn select_expression_supports_label_bound_selects() {
    let expression = select_expression(10.0, 20.0, "New York").unwrap();
    assert!(expression.contains("label.control instanceof HTMLSelectElement"));
    assert!(expression.contains("exact option value or label text"));
    assert!(expression.contains("dispatchEvent(new Event('change'"));
}

#[test]
fn checkable_state_expression_supports_labels_and_roles() {
    let expression = checkable_state_expression(10.0, 20.0);
    assert!(expression.contains("label.control instanceof HTMLInputElement"));
    assert!(expression.contains("[role=\"checkbox\"], [role=\"radio\"]"));
    assert!(expression.contains("Target is not a checkbox or radio control"));
}
