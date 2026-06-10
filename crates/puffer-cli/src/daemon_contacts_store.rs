//! Saved-contact JSON persistence helpers.

use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use puffer_subscriptions::{normalize_contact_ids, ContactProposal, SavedContact};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const CONTACT_STORE_VERSION: u32 = 1;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(super) struct ContactStoreFile {
    #[serde(default = "contact_store_version")]
    pub(super) version: u32,
    #[serde(default)]
    pub(super) contacts: Vec<SavedContact>,
    #[serde(default)]
    pub(super) proposals: Vec<ContactProposal>,
}

impl Default for ContactStoreFile {
    fn default() -> Self {
        Self {
            version: CONTACT_STORE_VERSION,
            contacts: Vec::new(),
            proposals: Vec::new(),
        }
    }
}

/// Loads saved contacts from the workspace runtime store.
pub(super) fn load_store(paths: &ConfigPaths) -> Result<ContactStoreFile> {
    let path = contacts_path(paths);
    if !path.exists() {
        return Ok(ContactStoreFile::default());
    }
    let raw = std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(ContactStoreFile::default());
    }
    let mut store =
        serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
    sanitize_store(&mut store);
    Ok(store)
}

/// Saves contacts to the workspace runtime store through a temporary file.
pub(super) fn save_store(paths: &ConfigPaths, store: &ContactStoreFile) -> Result<()> {
    let path = contacts_path(paths);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let mut persisted = store.clone();
    normalize_store_version(&mut persisted);
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&persisted)?)
        .with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))
}

/// Replaces the last inferred contact proposals in the workspace store.
pub(super) fn save_proposals(
    paths: &ConfigPaths,
    proposals: Vec<ContactProposal>,
) -> Result<Vec<ContactProposal>> {
    let mut store = load_store(paths)?;
    store.proposals = proposals;
    sanitize_store(&mut store);
    let stored = store.proposals.clone();
    save_store(paths, &store)?;
    Ok(stored)
}

/// Removes inferred proposals that overlap saved contact ids.
pub(super) fn prune_proposals_for_contact_ids(
    store: &mut ContactStoreFile,
    contact_ids: &[String],
) {
    let saved_ids = normalize_contact_ids(contact_ids);
    if saved_ids.is_empty() {
        return;
    }
    store.proposals.retain(|proposal| {
        normalize_contact_ids(&proposal.contact_ids)
            .iter()
            .all(|proposal_id| !saved_ids.contains(proposal_id))
    });
}

/// Removes inferred proposals that overlap any saved contact.
pub(super) fn prune_proposals_for_saved_contacts(store: &mut ContactStoreFile) {
    let contact_ids = store
        .contacts
        .iter()
        .flat_map(|contact| contact.contact_ids.iter().cloned())
        .collect::<Vec<_>>();
    prune_proposals_for_contact_ids(store, &contact_ids);
}

fn sanitize_store(store: &mut ContactStoreFile) {
    normalize_store_version(store);
    sanitize_contacts(&mut store.contacts);
    sanitize_proposals(&mut store.proposals);
    prune_proposals_for_saved_contacts(store);
}

fn normalize_store_version(store: &mut ContactStoreFile) {
    if store.version == 0 {
        store.version = CONTACT_STORE_VERSION;
    }
}

fn sanitize_contacts(contacts: &mut Vec<SavedContact>) {
    let mut sanitized: Vec<SavedContact> = Vec::new();
    for mut contact in std::mem::take(contacts) {
        contact.id = contact.id.trim().to_string();
        contact.name = contact.name.trim().to_string();
        contact.description = contact.description.trim().to_string();
        contact.avatar = trimmed_optional(contact.avatar);
        contact.contact_ids = normalize_contact_ids(&contact.contact_ids);
        if contact.contact_ids.is_empty() {
            continue;
        }
        if contact.id.is_empty() {
            contact.id = derived_contact_id(&contact.contact_ids);
        }
        if contact.name.is_empty() {
            contact.name = contact.contact_ids[0].clone();
        }
        if let Some(existing) = sanitized
            .iter_mut()
            .find(|existing| contact_ids_overlap(&existing.contact_ids, &contact.contact_ids))
        {
            merge_saved_contact(existing, contact);
        } else {
            sanitized.push(contact);
        }
    }
    *contacts = sanitized;
}

fn sanitize_proposals(proposals: &mut Vec<ContactProposal>) {
    proposals.retain_mut(|proposal| {
        proposal.name = proposal.name.trim().to_string();
        proposal.description = proposal.description.trim().to_string();
        proposal.avatar = trimmed_optional(proposal.avatar.take());
        proposal.contact_ids = normalize_contact_ids(&proposal.contact_ids);
        !proposal.name.is_empty() && !proposal.contact_ids.is_empty()
    });
}

fn merge_saved_contact(existing: &mut SavedContact, contact: SavedContact) {
    if existing.name.is_empty() {
        existing.name = contact.name;
    }
    if existing.description.is_empty() {
        existing.description = contact.description;
    }
    if existing.avatar.is_none() {
        existing.avatar = contact.avatar;
    }
    existing.contact_ids = normalize_contact_ids(
        existing
            .contact_ids
            .iter()
            .chain(contact.contact_ids.iter())
            .cloned(),
    );
}

fn contact_ids_overlap(left: &[String], right: &[String]) -> bool {
    let left = normalize_contact_ids(left);
    let right = normalize_contact_ids(right);
    left.iter().any(|id| right.contains(id))
}

fn trimmed_optional(value: Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn derived_contact_id(contact_ids: &[String]) -> String {
    let slug = contact_ids
        .first()
        .map(|id| {
            id.chars()
                .map(|ch| {
                    if ch.is_ascii_alphanumeric() {
                        ch.to_ascii_lowercase()
                    } else {
                        '-'
                    }
                })
                .collect::<String>()
                .trim_matches('-')
                .to_string()
        })
        .filter(|slug| !slug.is_empty())
        .unwrap_or_else(|| "contact".to_string());
    format!("contact-{slug}")
}

fn contacts_path(paths: &ConfigPaths) -> PathBuf {
    paths
        .workspace_config_dir
        .join("runtime")
        .join("contacts.json")
}

fn contact_store_version() -> u32 {
    CONTACT_STORE_VERSION
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_config::ConfigPaths;
    use std::path::Path;

    #[test]
    fn load_store_defaults_missing_version_and_proposals() {
        let temp = tempfile::tempdir().unwrap();
        let paths = test_config_paths(temp.path());
        let runtime_dir = paths.workspace_config_dir.join("runtime");
        std::fs::create_dir_all(&runtime_dir).unwrap();
        std::fs::write(runtime_dir.join("contacts.json"), r#"{"contacts":[]}"#).unwrap();

        let store = load_store(&paths).unwrap();

        assert_eq!(store.version, CONTACT_STORE_VERSION);
        assert!(store.proposals.is_empty());
    }

    #[test]
    fn save_store_writes_current_version_for_new_store() {
        let temp = tempfile::tempdir().unwrap();
        let paths = test_config_paths(temp.path());

        save_store(&paths, &ContactStoreFile::default()).unwrap();

        let raw = std::fs::read_to_string(paths.workspace_config_dir.join("runtime/contacts.json"))
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(value["version"], CONTACT_STORE_VERSION);
    }

    #[test]
    fn save_proposals_preserves_saved_contacts() {
        let temp = tempfile::tempdir().unwrap();
        let paths = test_config_paths(temp.path());
        let mut store = ContactStoreFile::default();
        store.contacts.push(SavedContact {
            id: "contact-alice".to_string(),
            name: "Alice".to_string(),
            description: "Project collaborator.".to_string(),
            avatar: None,
            contact_ids: vec!["telegram@alice".to_string()],
        });
        save_store(&paths, &store).unwrap();

        save_proposals(
            &paths,
            vec![ContactProposal {
                name: "Bob".to_string(),
                description: "Frequent project contact.".to_string(),
                avatar: Some("data:image/jpeg;base64,ZmFrZQ==".to_string()),
                contact_ids: vec!["telegram@bob".to_string()],
            }],
        )
        .unwrap();

        let store = load_store(&paths).unwrap();

        assert_eq!(store.contacts.len(), 1);
        assert_eq!(store.proposals.len(), 1);
        assert_eq!(
            store.proposals[0].contact_ids,
            vec!["telegram@bob".to_string()]
        );
    }

    #[test]
    fn load_store_drops_legacy_saved_contact_proposals() {
        let temp = tempfile::tempdir().unwrap();
        let paths = test_config_paths(temp.path());
        let runtime_dir = paths.workspace_config_dir.join("runtime");
        std::fs::create_dir_all(&runtime_dir).unwrap();
        std::fs::write(
            runtime_dir.join("contacts.json"),
            serde_json::to_vec_pretty(&ContactStoreFile {
                version: 1,
                contacts: vec![SavedContact {
                    id: "contact-alice".to_string(),
                    name: "Alice".to_string(),
                    description: "Project collaborator.".to_string(),
                    avatar: None,
                    contact_ids: vec!["telegram@alice".to_string()],
                }],
                proposals: vec![ContactProposal {
                    name: "Alice".to_string(),
                    description: "Already saved.".to_string(),
                    avatar: None,
                    contact_ids: vec!["telegram@alice".to_string()],
                }],
            })
            .unwrap(),
        )
        .unwrap();

        let store = load_store(&paths).unwrap();

        assert!(store.proposals.is_empty());
    }

    #[test]
    fn load_store_sanitizes_legacy_contacts_and_proposals() {
        let temp = tempfile::tempdir().unwrap();
        let paths = test_config_paths(temp.path());
        let runtime_dir = paths.workspace_config_dir.join("runtime");
        std::fs::create_dir_all(&runtime_dir).unwrap();
        std::fs::write(
            runtime_dir.join("contacts.json"),
            serde_json::to_vec_pretty(&ContactStoreFile {
                version: 1,
                contacts: vec![SavedContact {
                    id: " contact-alice ".to_string(),
                    name: " Alice ".to_string(),
                    description: " Project collaborator. ".to_string(),
                    avatar: Some("  ".to_string()),
                    contact_ids: vec![
                        " Telegram@@Alice ".to_string(),
                        "telegram@12345".to_string(),
                        "Google@Alice@Example.COM".to_string(),
                    ],
                }],
                proposals: vec![
                    ContactProposal {
                        name: " Bob ".to_string(),
                        description: " Unsaved collaborator. ".to_string(),
                        avatar: Some(" data:image/jpeg;base64,ZmFrZQ== ".to_string()),
                        contact_ids: vec![" Telegram@@Bob ".to_string()],
                    },
                    ContactProposal {
                        name: " Invalid ".to_string(),
                        description: "Invalid proposal.".to_string(),
                        avatar: None,
                        contact_ids: vec!["not-a-contact".to_string()],
                    },
                    ContactProposal {
                        name: " Alice ".to_string(),
                        description: "Already saved.".to_string(),
                        avatar: None,
                        contact_ids: vec!["telegram@alice".to_string()],
                    },
                ],
            })
            .unwrap(),
        )
        .unwrap();

        let store = load_store(&paths).unwrap();

        assert_eq!(store.contacts.len(), 1);
        assert_eq!(store.contacts[0].id, "contact-alice");
        assert_eq!(store.contacts[0].name, "Alice");
        assert_eq!(store.contacts[0].description, "Project collaborator.");
        assert_eq!(store.contacts[0].avatar, None);
        assert_eq!(
            store.contacts[0].contact_ids,
            vec![
                "google@alice@example.com".to_string(),
                "telegram@alice".to_string()
            ]
        );
        assert_eq!(store.proposals.len(), 1);
        assert_eq!(store.proposals[0].name, "Bob");
        assert_eq!(
            store.proposals[0].contact_ids,
            vec!["telegram@bob".to_string()]
        );
        assert_eq!(
            store.proposals[0].avatar.as_deref(),
            Some("data:image/jpeg;base64,ZmFrZQ==")
        );
    }

    #[test]
    fn load_store_merges_legacy_overlapping_saved_contacts() {
        let temp = tempfile::tempdir().unwrap();
        let paths = test_config_paths(temp.path());
        let runtime_dir = paths.workspace_config_dir.join("runtime");
        std::fs::create_dir_all(&runtime_dir).unwrap();
        std::fs::write(
            runtime_dir.join("contacts.json"),
            serde_json::to_vec_pretty(&ContactStoreFile {
                version: 1,
                contacts: vec![
                    SavedContact {
                        id: "contact-alice".to_string(),
                        name: "Alice".to_string(),
                        description: "".to_string(),
                        avatar: None,
                        contact_ids: vec!["Telegram@@Alice".to_string()],
                    },
                    SavedContact {
                        id: "contact-alice-email".to_string(),
                        name: "Alice Email".to_string(),
                        description: "Email identity.".to_string(),
                        avatar: Some("data:image/jpeg;base64,ZmFrZQ==".to_string()),
                        contact_ids: vec![
                            "telegram@alice".to_string(),
                            "google@alice@example.com".to_string(),
                        ],
                    },
                ],
                proposals: Vec::new(),
            })
            .unwrap(),
        )
        .unwrap();

        let store = load_store(&paths).unwrap();

        assert_eq!(store.contacts.len(), 1);
        assert_eq!(store.contacts[0].id, "contact-alice");
        assert_eq!(store.contacts[0].name, "Alice");
        assert_eq!(store.contacts[0].description, "Email identity.");
        assert_eq!(
            store.contacts[0].avatar.as_deref(),
            Some("data:image/jpeg;base64,ZmFrZQ==")
        );
        assert_eq!(
            store.contacts[0].contact_ids,
            vec![
                "google@alice@example.com".to_string(),
                "telegram@alice".to_string()
            ]
        );
    }

    #[test]
    fn load_store_drops_unroutable_saved_contacts_and_repairs_blank_fields() {
        let temp = tempfile::tempdir().unwrap();
        let paths = test_config_paths(temp.path());
        let runtime_dir = paths.workspace_config_dir.join("runtime");
        std::fs::create_dir_all(&runtime_dir).unwrap();
        std::fs::write(
            runtime_dir.join("contacts.json"),
            serde_json::to_vec_pretty(&ContactStoreFile {
                version: 1,
                contacts: vec![
                    SavedContact {
                        id: " ".to_string(),
                        name: " ".to_string(),
                        description: " Legacy row with usable routing. ".to_string(),
                        avatar: Some(" ".to_string()),
                        contact_ids: vec![" Slack@U123 ".to_string()],
                    },
                    SavedContact {
                        id: "contact-invalid".to_string(),
                        name: "Invalid".to_string(),
                        description: "No usable contact ids.".to_string(),
                        avatar: None,
                        contact_ids: vec!["not-a-contact".to_string()],
                    },
                ],
                proposals: Vec::new(),
            })
            .unwrap(),
        )
        .unwrap();

        let store = load_store(&paths).unwrap();

        assert_eq!(store.contacts.len(), 1);
        assert_eq!(store.contacts[0].id, "contact-slack-u123");
        assert_eq!(store.contacts[0].name, "slack@U123");
        assert_eq!(
            store.contacts[0].description,
            "Legacy row with usable routing."
        );
        assert_eq!(store.contacts[0].avatar, None);
        assert_eq!(store.contacts[0].contact_ids, vec!["slack@U123"]);
    }

    #[test]
    fn save_proposals_sanitizes_returned_and_persisted_values() {
        let temp = tempfile::tempdir().unwrap();
        let paths = test_config_paths(temp.path());

        let stored = save_proposals(
            &paths,
            vec![
                ContactProposal {
                    name: " Bob ".to_string(),
                    description: " Frequent collaborator. ".to_string(),
                    avatar: Some(" data:image/jpeg;base64,ZmFrZQ== ".to_string()),
                    contact_ids: vec![
                        " Telegram@@Bob ".to_string(),
                        "telegram@12345".to_string(),
                        "Google@Bob@Example.COM".to_string(),
                    ],
                },
                ContactProposal {
                    name: "Invalid".to_string(),
                    description: "Invalid proposal.".to_string(),
                    avatar: None,
                    contact_ids: vec!["not-a-contact".to_string()],
                },
            ],
        )
        .unwrap();

        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].name, "Bob");
        assert_eq!(stored[0].description, "Frequent collaborator.");
        assert_eq!(
            stored[0].avatar.as_deref(),
            Some("data:image/jpeg;base64,ZmFrZQ==")
        );
        assert_eq!(
            stored[0].contact_ids,
            vec![
                "google@bob@example.com".to_string(),
                "telegram@bob".to_string()
            ]
        );

        let reloaded = load_store(&paths).unwrap();

        assert_eq!(reloaded.proposals, stored);
    }

    #[test]
    fn save_proposals_drops_saved_contact_overlaps() {
        let temp = tempfile::tempdir().unwrap();
        let paths = test_config_paths(temp.path());
        let mut store = ContactStoreFile::default();
        store.contacts.push(SavedContact {
            id: "contact-alice".to_string(),
            name: "Alice".to_string(),
            description: "Project collaborator.".to_string(),
            avatar: None,
            contact_ids: vec!["telegram@alice".to_string()],
        });
        save_store(&paths, &store).unwrap();

        let stored = save_proposals(
            &paths,
            vec![
                ContactProposal {
                    name: "Alice".to_string(),
                    description: "Already saved.".to_string(),
                    avatar: None,
                    contact_ids: vec!["telegram@alice".to_string()],
                },
                ContactProposal {
                    name: "Bob".to_string(),
                    description: "Unsaved collaborator.".to_string(),
                    avatar: None,
                    contact_ids: vec!["telegram@bob".to_string()],
                },
            ],
        )
        .unwrap();

        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].name, "Bob");
        assert_eq!(load_store(&paths).unwrap().proposals, stored);
    }

    #[test]
    fn prune_proposals_for_contact_ids_removes_overlapping_proposals() {
        let mut store = ContactStoreFile {
            version: 1,
            contacts: Vec::new(),
            proposals: vec![
                ContactProposal {
                    name: "Alice".to_string(),
                    description: "Frequent launch collaborator.".to_string(),
                    avatar: None,
                    contact_ids: vec!["Telegram@Alice".to_string()],
                },
                ContactProposal {
                    name: "Bob".to_string(),
                    description: "Separate support contact.".to_string(),
                    avatar: None,
                    contact_ids: vec!["telegram@bob".to_string()],
                },
            ],
        };

        prune_proposals_for_contact_ids(&mut store, &["telegram@alice".to_string()]);

        assert_eq!(store.proposals.len(), 1);
        assert_eq!(store.proposals[0].name, "Bob");
    }

    fn test_config_paths(root: &Path) -> ConfigPaths {
        ConfigPaths {
            workspace_root: root.to_path_buf(),
            workspace_config_dir: root.join(".puffer"),
            user_config_dir: root.join("user-puffer"),
            builtin_resources_dir: root.join("resources"),
        }
    }
}
