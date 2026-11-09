use super::{ObjectRecord, Observation, RecordError, RunData, RunLog};
use crate::context::LLMContext;
use serde_json::Value;
use std::{
    fs::{self, OpenOptions},
    io::{BufReader, Write},
    path::Path,
    sync::{Arc, Mutex, MutexGuard},
    time::{SystemTime, UNIX_EPOCH},
};

impl RunLog {
    pub fn new(id: String, experiment: String, config: Value) -> Self {
        Self {
            inner: Arc::new(Mutex::new(RunData {
                id,
                experiment,
                config,
                objects: Vec::new(),
                records: Vec::new(),
                snapshots: Vec::new(),
                summary: Value::Null,
            })),
        }
    }

    fn lock(&self) -> Result<MutexGuard<'_, RunData>, RecordError> {
        self.inner.lock().map_err(|_| RecordError::Poisoned)
    }

    pub fn register(&self, id: &str, kind: &str, label: &str) -> Result<(), RecordError> {
        if id.is_empty() || kind.is_empty() {
            return Err(RecordError::Invalid(
                "object id and kind must be nonempty".into(),
            ));
        }
        let mut data = self.lock()?;

        let object = ObjectRecord {
            id: id.into(),
            kind: kind.into(),
            label: label.into(),
        };
        if let Some(existing) = data.objects.iter().find(|object| object.id == id) {
            if existing == &object {
                return Ok(());
            }
            return Err(RecordError::Invalid(format!(
                "object {id} already has different metadata"
            )));
        }
        data.objects.push(object);
        Ok(())
    }
    pub(super) fn validate_source(
        &self,
        object_id: &str,
        cause: Option<&str>,
    ) -> Result<(), RecordError> {
        let data = self.lock()?;
        if !data.objects.iter().any(|object| object.id == object_id) {
            return Err(RecordError::Invalid(format!(
                "object {object_id} is not registered"
            )));
        }
        if cause.is_some_and(|cause| !data.records.iter().any(|record| record.id == cause)) {
            return Err(RecordError::Invalid(
                "call cause does not identify an earlier observation".into(),
            ));
        }
        Ok(())
    }

    pub fn observe(
        &self,
        object_id: &str,
        kind: &str,
        cause: Option<&str>,
        value: Value,
    ) -> Result<String, RecordError> {
        if kind.is_empty() {
            return Err(RecordError::Invalid(
                "observation kind must be nonempty".into(),
            ));
        }
        let mut data = self.lock()?;
        if !data.objects.iter().any(|object| object.id == object_id) {
            return Err(RecordError::Invalid(format!(
                "object {object_id} is not registered"
            )));
        }
        if let Some(cause) = cause {
            if !data.records.iter().any(|record| record.id == cause) {
                return Err(RecordError::Invalid(format!(
                    "cause {cause} does not identify an earlier observation"
                )));
            }
        }
        let sequence = u64::try_from(data.records.len())
            .map_err(|_| RecordError::Invalid("observation sequence exhausted".into()))?
            .checked_add(1)
            .ok_or_else(|| RecordError::Invalid("observation sequence exhausted".into()))?;
        let id = format!("{}/record/{sequence}", data.id);
        let timestamp_ms = u64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_| RecordError::Clock)?
                .as_millis(),
        )
        .map_err(|_| RecordError::Invalid("timestamp exceeds u64 milliseconds".into()))?;
        let run_id = data.id.clone();
        data.records.push(Observation {
            id: id.clone(),
            sequence,
            run_id,
            object_id: object_id.into(),
            cause: cause.map(str::to_owned),
            timestamp_ms,
            kind: kind.into(),
            data: value,
        });
        Ok(id)
    }

    pub fn snapshot(&self, context: &LLMContext) -> Result<(), RecordError> {
        let mut data = self.lock()?;
        if let Some(existing) = data
            .snapshots
            .iter()
            .find(|snapshot| snapshot.id == context.id && snapshot.version == context.version)
        {
            if existing == context {
                return Ok(());
            }
            return Err(RecordError::Invalid(format!(
                "context {} version {} was changed after capture",
                context.id, context.version
            )));
        }
        data.snapshots.push(context.clone());
        Ok(())
    }

    pub fn data(&self) -> Result<RunData, RecordError> {
        Ok(self.lock()?.clone())
    }

    pub fn set_summary(&self, summary: Value) -> Result<(), RecordError> {
        self.lock()?.summary = summary;
        Ok(())
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), RecordError> {
        let path = path.as_ref();
        let filename = path
            .file_name()
            .ok_or_else(|| RecordError::Invalid("run path must name a file".into()))?;
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(parent)?;
        let mut temp_name = filename.to_os_string();
        temp_name.push(format!(".{}.tmp", uuid::Uuid::new_v4()));
        let temp = parent.join(temp_name);
        let bytes = serde_json::to_vec_pretty(&self.data()?)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        let result = (|| -> Result<(), RecordError> {
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::rename(&temp, path)?;
            Ok(())
        })();
        if result.is_err() && temp.exists() {
            fs::remove_file(&temp)?;
        }
        result
    }

    pub(super) fn call_context(
        &self,
        cause: Option<&str>,
    ) -> Result<(Option<String>, Option<u64>), RecordError> {
        let data = self.lock()?;
        match cause {
            None => Ok((None, None)),
            Some(cause) => {
                let observation = data
                    .records
                    .iter()
                    .find(|record| record.id == cause)
                    .ok_or_else(|| RecordError::Invalid(format!("unknown call cause {cause}")))?;
                if observation.kind != "call" {
                    return Ok((None, None));
                }
                let call: super::CallRecord = serde_json::from_value(observation.data.clone())?;
                Ok((call.context_id, call.context_version))
            }
        }
    }
}

impl RunData {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, RecordError> {
        Ok(serde_json::from_reader(BufReader::new(fs::File::open(
            path,
        )?))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::ChatMessage;
    use serde_json::json;

    #[test]
    fn snapshots_are_owned_and_same_version_cannot_be_rewritten() {
        let log = RunLog::new("run".into(), "test".into(), json!({"configuration":1}));
        let mut context = LLMContext::new("c", "system");
        log.snapshot(&context).expect("snapshot");
        context.append(ChatMessage::User {
            content: "later".into(),
        });
        log.snapshot(&context).expect("new snapshot");
        assert!(log.data().expect("run").snapshots[0].messages.is_empty());
        context.system_prompt = "unversioned mutation".into();
        assert!(log.snapshot(&context).is_err());
        let mut copy = log.data().expect("run");
        copy.snapshots[0].system_prompt = "copy change".into();
        assert_eq!(
            log.data().expect("run").snapshots[0].system_prompt,
            "system"
        );
    }

    #[test]
    fn file_round_trip_preserves_extensible_objects_and_all_data() {
        let log = RunLog::new("run".into(), "test".into(), json!({"configuration":1}));
        log.register("custom", "custom_resource", "Custom resource")
            .expect("register");
        let cause = log
            .observe("custom", "custom_event", None, json!({"version":1}))
            .expect("observe");
        log.observe(
            "custom",
            "custom_delivery",
            Some(&cause),
            json!({"payload":[1,2]}),
        )
        .expect("observe causal");
        log.snapshot(&LLMContext::new("c", "system"))
            .expect("snapshot");
        log.set_summary(json!({"result":"ok"})).expect("summary");
        let directory = std::env::temp_dir().join(format!("layer-cake-{}", uuid::Uuid::new_v4()));
        let path = directory.join("nested/run.json");
        log.save(&path).expect("atomic save");
        let restored = RunData::load(&path).expect("load");
        fs::remove_dir_all(directory).expect("remove test directory");
        assert_eq!(restored, log.data().expect("run"));
    }

    #[test]
    fn clones_share_state_and_concurrent_observations_have_total_order() {
        let log = RunLog::new("run".into(), "test".into(), Value::Null);
        log.register("object", "test", "Object").expect("register");
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let log = log.clone();
                std::thread::spawn(move || {
                    for _ in 0..10 {
                        log.observe("object", "event", None, Value::Null)
                            .expect("observe");
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().expect("worker");
        }
        let data = log.data().expect("run");
        assert_eq!(data.records.len(), 40);
        assert!(
            data.records
                .iter()
                .enumerate()
                .all(|(index, record)| record.sequence == index as u64 + 1)
        );
    }
}
