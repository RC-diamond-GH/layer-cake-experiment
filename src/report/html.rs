use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

use serde::Serialize;
use serde_json::json;

use super::ReportError;
use crate::record::{CallRecord, RunData};

/// Render all data as escaped HTML text; no run-controlled content enters scripts or markup.
pub fn html(data: &RunData) -> Result<String, ReportError> {
    let mut out = String::from(
        r##"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'">
<title>Layer Cake run report</title><style>
:root{color-scheme:light dark;font-family:system-ui,sans-serif;line-height:1.5}body{max-width:1100px;margin:auto;padding:1.5rem}h1,h2,h3{line-height:1.2}a{color:light-dark(#174db5,#99bdff)}pre{overflow:auto;white-space:pre-wrap;overflow-wrap:anywhere;background:light-dark(#f1f3f6,#20252d);padding:1rem;border-radius:.3rem}article,section.panel{border:1px solid #8886;border-radius:.4rem;padding:1rem;margin:1rem 0}article header{display:flex;gap:.7rem;flex-wrap:wrap}small,.muted{color:light-dark(#4c5664,#b6c0ce)}details{margin:.5rem 0}summary{cursor:pointer;font-weight:600}label{display:inline-flex;gap:.5rem;margin:.5rem 1rem .5rem 0;align-items:center}input,select{max-width:100%;font:inherit;padding:.3rem}nav{display:flex;gap:1rem;flex-wrap:wrap}.tag{border:1px solid #8888;border-radius:.25rem;padding:0 .3rem}[hidden]{display:none!important}.controls{position:sticky;top:0;background:Canvas;border:1px solid #8886;padding:.5rem;z-index:1}table{border-collapse:collapse;width:100%}td,th{text-align:left;vertical-align:top;border-bottom:1px solid #8886;padding:.5rem;overflow-wrap:anywhere}
*{box-sizing:border-box}body{overflow-wrap:anywhere}label{max-width:100%;flex-wrap:wrap}input,select{min-width:0;max-width:100%}
</style></head><body><h1>Layer Cake run report</h1>
<nav><a href="#metadata">Run metadata</a><a href="#timeline">Timeline</a><a href="#contexts">Context snapshots</a><a href="#objects">Objects</a></nav>
"##,
    );
    out.push_str("<section id=\"metadata\" class=\"panel\"><h2>Run metadata</h2><p>Run <strong>");
    escape(&mut out, &data.id);
    out.push_str("</strong> · Experiment <strong>");
    escape(&mut out, &data.experiment);
    out.push_str("</strong></p>");
    let cutoff = data.records.iter().max_by_key(|record| record.sequence);
    out.push_str("<p><strong>Report cutoff:</strong> ");
    if let Some(record) = cutoff {
        write!(
            &mut out,
            "sequence {} · timestamp {} ms · record ",
            record.sequence, record.timestamp_ms
        )
        .expect("writing to String cannot fail");
        escape(&mut out, &record.id);
    } else {
        out.push_str("no observations");
    }
    out.push_str(". This document contains only the records available at this cutoff.</p><p><strong>Recorded status:</strong> ");
    if let Some(status) = data
        .summary
        .get("status")
        .or_else(|| data.summary.get("state"))
    {
        escape(&mut out, &serde_json::to_string(status)?);
    } else {
        out.push_str("see complete summary below; no explicit status field recorded");
    }
    out.push_str("</p>");
    details(&mut out, "Configuration", &data.config, true)?;
    details(
        &mut out,
        "Result summary / controller state",
        &data.summary,
        true,
    )?;
    out.push_str("</section><h2 id=\"timeline\">Timeline</h2><div class=\"controls\"><label>Object <select id=\"object-filter\"><option value=\"\">All objects</option>");
    let mut objects: BTreeMap<&str, &str> = data
        .objects
        .iter()
        .map(|object| (object.id.as_str(), object.label.as_str()))
        .collect();
    for record in &data.records {
        objects
            .entry(&record.object_id)
            .or_insert(&record.object_id);
    }
    for (id, label) in &objects {
        out.push_str("<option value=\"");
        escape(&mut out, id);
        out.push_str("\">");
        escape(&mut out, label);
        out.push_str(" (");
        escape(&mut out, id);
        out.push_str(")</option>");
    }
    out.push_str("</select></label><label>Search <input id=\"search\" type=\"search\" placeholder=\"Search complete record text\"></label><span id=\"visible-count\" role=\"status\"></span></div><div id=\"records\">");
    let record_links: BTreeMap<&str, usize> = data
        .records
        .iter()
        .enumerate()
        .map(|(index, record)| (record.id.as_str(), index))
        .collect();
    let mut records: Vec<_> = data.records.iter().enumerate().collect();
    records.sort_by_key(|(_, record)| record.sequence);
    for (index, record) in records {
        write!(&mut out, "<article id=\"record-{index}\" data-object=\"")
            .expect("writing to String cannot fail");
        escape(&mut out, &record.object_id);
        out.push_str("\"><header><strong>");
        write!(&mut out, "#{}", record.sequence).expect("writing to String cannot fail");
        out.push_str("</strong><span class=\"tag\">");
        escape(&mut out, &record.kind);
        out.push_str("</span><span>");
        escape(&mut out, &record.object_id);
        out.push_str("</span><small>");
        write!(&mut out, "{} ms", record.timestamp_ms).expect("writing to String cannot fail");
        out.push_str("</small></header><p>Record ");
        escape(&mut out, &record.id);
        if let Some(cause) = &record.cause {
            out.push_str(" · Cause ");
            if let Some(cause_index) = record_links.get(cause.as_str()) {
                write!(&mut out, "<a href=\"#record-{cause_index}\">")
                    .expect("writing to String cannot fail");
                escape(&mut out, cause);
                out.push_str("</a>");
            } else {
                escape(&mut out, cause);
            }
        }
        out.push_str("</p>");
        if record.kind == "call" {
            let call: CallRecord =
                serde_json::from_value(record.data.clone()).map_err(|error| {
                    ReportError::InvalidCall {
                        record_id: record.id.clone(),
                        message: error.to_string(),
                    }
                })?;
            out.push_str("<h3>");
            escape(&mut out, &call.call_kind);
            out.push_str(" call</h3><p>");
            write!(&mut out, "Elapsed {} ms", call.elapsed_ms)
                .expect("writing to String cannot fail");
            if let Some(tool_call_id) = &call.tool_call_id {
                out.push_str(" · Tool call ");
                escape(&mut out, tool_call_id);
            }
            if let (Some(id), Some(version)) = (&call.context_id, call.context_version) {
                out.push_str(" · Context ");
                escape(&mut out, id);
                write!(&mut out, " version {version}").expect("writing to String cannot fail");
                if let Some(snapshot_index) = data
                    .snapshots
                    .iter()
                    .position(|snapshot| snapshot.id == *id && snapshot.version == version)
                {
                    write!(
                        &mut out,
                        " · <a href=\"#context-{snapshot_index}\">Open exact snapshot</a>"
                    )
                    .expect("writing to String cannot fail");
                } else {
                    out.push_str(" (snapshot absent from saved run)");
                }
            }
            out.push_str("</p>");
            details(
                &mut out,
                "Complete request (context, parameters or tool arguments and visibility)",
                &call.input,
                true,
            )?;
            details(&mut out, "Complete returned output", &call.output, true)?;
            details(&mut out, "Complete returned error", &call.error, true)?;
        } else {
            details(&mut out, "Observation data", &record.data, true)?;
        }
        details(
            &mut out,
            "Complete observation metadata and payload",
            record,
            false,
        )?;
        out.push_str("</article>");
    }
    out.push_str("</div><h2 id=\"contexts\">Context snapshots</h2><p>Immutable inputs, indexed by context identity and original version. Sizes below are UTF-8 JSON bytes, not token estimates.</p>");
    let mut seen = BTreeSet::new();
    for (index, snapshot) in data.snapshots.iter().enumerate() {
        write!(
            &mut out,
            "<section id=\"context-{index}\" class=\"panel\"><h3>"
        )
        .expect("writing to String cannot fail");
        escape(&mut out, &snapshot.id);
        write!(
            &mut out,
            " · version {}</h3><p>{} messages · {} tools · {} UTF-8 JSON bytes</p>",
            snapshot.version,
            snapshot.messages.len(),
            snapshot.tools.len(),
            serde_json::to_vec(snapshot)?.len()
        )
        .expect("writing to String cannot fail");
        if !seen.insert((&snapshot.id, snapshot.version)) {
            out.push_str("<p>Repeated identity/version in saved snapshot list.</p>");
        }
        details(&mut out, "Full context snapshot", snapshot, true)?;
        out.push_str("</section>");
    }
    out.push_str("<section id=\"objects\" class=\"panel\"><h2>Object index</h2>");
    details(&mut out, "Registered objects", &data.objects, true)?;
    details(
        &mut out,
        "Report metadata",
        &json!({"run_id":data.id,"experiment":data.experiment,"observation_count":data.records.len(),"snapshot_count":data.snapshots.len(),"cutoff_sequence":cutoff.map(|record|record.sequence),"cutoff_record":cutoff.map(|record|&record.id)}),
        true,
    )?;
    out.push_str(r##"</section><script>
'use strict';
const objectFilter=document.getElementById('object-filter');
const search=document.getElementById('search');
const records=Array.from(document.querySelectorAll('#records article'));
function filter(){const object=objectFilter.value;const query=search.value.toLocaleLowerCase();let visible=0;for(const record of records){record.hidden=(object!==''&&record.dataset.object!==object)||(query!==''&&!record.textContent.toLocaleLowerCase().includes(query));if(!record.hidden)visible++;}document.getElementById('visible-count').textContent=visible+' / '+records.length+' observations';}
objectFilter.addEventListener('change',filter);search.addEventListener('input',filter);
function revealTarget(){const target=document.getElementById(location.hash.slice(1));if(target&&target.matches('#records article')){objectFilter.value='';search.value='';filter();target.scrollIntoView();}}
window.addEventListener('hashchange',revealTarget);filter();revealTarget();
</script></body></html>"##);
    Ok(out)
}

fn details<T: Serialize + ?Sized>(
    out: &mut String,
    label: &str,
    value: &T,
    open: bool,
) -> Result<(), ReportError> {
    out.push_str(if open {
        "<details open><summary>"
    } else {
        "<details><summary>"
    });
    escape(out, label);
    out.push_str("</summary><pre>");
    escape(out, &serde_json::to_string_pretty(value)?);
    out.push_str("</pre></details>");
    Ok(())
}

fn escape(out: &mut String, value: &str) {
    for character in value.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(character),
        }
    }
}
