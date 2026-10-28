use super::actions::ACTIONS;

pub fn description(id: &str) -> &'static str {
    ACTIONS.iter().find(|a| a.id == id).map_or("", |a| a.desc.t())
}
