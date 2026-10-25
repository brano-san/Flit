use super::App;
use crate::provider::OpId;

impl App {
    pub(super) fn hashed(&mut self, _id: OpId, _result: Result<Vec<String>, String>) {}
}
