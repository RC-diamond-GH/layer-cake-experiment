use super::{CliResult, View};
use layer_cake::record::RunData;
use serde_json::{Value, json};

pub fn select(
    data: &RunData,
    view: View,
    id: Option<&str>,
    version: Option<u64>,
    object: Option<&str>,
    kind: Option<&str>,
) -> CliResult<Value> {
    Ok(match view {
        View::State => {
            json!({"id":data.id,"experiment":data.experiment,"config":data.config,"summary":data.summary,"cutoff":data.records.last().map(|r| r.sequence)})
        }
        View::Objects => serde_json::to_value(&data.objects)?,
        View::Records | View::Calls => {
            let calls_only = matches!(view, View::Calls);
            let records: Vec<_> = data
                .records
                .iter()
                .filter(|r| {
                    (!calls_only || r.kind == "call")
                        && id.is_none_or(|id| r.id == id)
                        && object.is_none_or(|id| r.object_id == id)
                        && kind.is_none_or(|kind| r.kind == kind)
                })
                .collect();
            if id.is_some() && records.is_empty() {
                return Err("record id not found".into());
            }
            serde_json::to_value(records)?
        }
        View::Context => {
            let snapshot = data
                .snapshots
                .iter()
                .filter(|s| id.is_none_or(|id| s.id == id))
                .filter(|s| version.is_none_or(|version| s.version == version))
                .max_by_key(|s| s.version)
                .ok_or("context id/version not found")?;
            serde_json::to_value(snapshot)?
        }
    })
}
