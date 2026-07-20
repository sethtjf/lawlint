#[cfg(feature = "sourcing")]
mod app {

    use lawlint_eval::sourcing::segment;
    use lawlint_eval::{Label, Sample};
    use serde::Deserialize;
    use serde_json::Value;
    use std::collections::HashSet;
    use std::fs::{self, OpenOptions};
    use std::io::Write;
    use std::thread::sleep;
    use std::time::Duration;

    #[derive(Debug, Deserialize)]
    struct Volume {
        volume_number: String,
        publication_year: Option<i32>,
    }

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let output = "crates/lawlint-eval/corpus/corpus.jsonl";
        let reporters = ["f3d", "f-supp-3d"];
        let mut passages = Vec::new();
        for reporter in reporters {
            let volumes: Vec<Volume> = get_json(&format!(
                "https://static.case.law/{reporter}/VolumesMetadata.json"
            ))?;
            for volume in volumes
                .into_iter()
                .filter(|volume| {
                    volume
                        .publication_year
                        .is_some_and(|year| (2000..=2019).contains(&year))
                })
                .take(12)
            {
                let cases: Vec<Value> = get_json(&format!(
                    "https://static.case.law/{reporter}/{}/CasesMetadata.json",
                    volume.volume_number
                ))?;
                for case in cases {
                    if passages.len() >= 137 {
                        break;
                    }
                    let date = case["decision_date"].as_str().unwrap_or_default();
                    if !(date.starts_with("200") || date.starts_with("201")) {
                        continue;
                    }
                    let file_name = match case["file_name"].as_str() {
                        Some(name) => name,
                        None => continue,
                    };
                    let document: Value = get_json(&format!(
                        "https://static.case.law/{reporter}/{}/cases/{file_name}.json",
                        volume.volume_number
                    ))?;
                    let opinion = document
                        .pointer("/casebody/opinions/0/text")
                        .or_else(|| document.pointer("/casebody/data/opinions/0/text"))
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    for text in segment(opinion, 100, 500)
                        .into_iter()
                        .filter(|text| text.split_whitespace().count() <= 500)
                    {
                        if !text.chars().next().is_some_and(char::is_uppercase)
                            || starts_with_citation_fragment(&text)
                            || has_ocr_fragment(&text)
                        {
                            continue;
                        }
                        passages.push(Sample {
                            id: format!("human-cap-{:06}", passages.len() + 1),
                            label: Label::Human,
                            word_count: Some(text.split_whitespace().count()),
                            text,
                            source: "caselaw-access-project".to_string(),
                            register: "opinion".to_string(),
                            era: Some("pre-2020".to_string()),
                            date: Some(date.to_string()),
                            court: case["court"]["name"].as_str().map(str::to_string),
                            model: None,
                            prompt_style: None,
                            pair_id: Some(format!("pair-cap-{:06}", passages.len() + 1)),
                            split: None,
                        });
                        if passages.len() >= 137 {
                            break;
                        }
                    }
                    sleep(Duration::from_millis(100));
                }
                if passages.len() >= 137 {
                    break;
                }
            }
            if passages.len() >= 137 {
                break;
            }
        }
        let mut seen = HashSet::new();
        passages.retain(|sample| {
            let key = sample
                .text
                .split_whitespace()
                .take(35)
                .collect::<Vec<_>>()
                .join(" ")
                .to_ascii_lowercase();
            seen.insert(key)
        });
        append_samples(output, passages)?;
        Ok(())
    }

    fn starts_with_citation_fragment(text: &str) -> bool {
        [
            "See ",
            "Id. ",
            "Schoonejongen,",
            "Rec. ",
            "Or.Rev.Stat.",
            "Mt. Healthy,",
        ]
        .iter()
        .any(|prefix| text.starts_with(prefix))
    }

    fn has_ocr_fragment(text: &str) -> bool {
        [
            "eviden-tiary",
            "formu-lae",
            "nonat-tainment",
            "sen-fencing",
            "re-fleets",
            "ag-grievement",
            "spe-val",
            "uro-cedural",
            "non-diseiplinary",
            "excluda-ble",
            "pri-ma",
            "eontract-i",
            "mini-mus",
            "exac-tions",
            "es-toppel",
        ]
        .iter()
        .any(|fragment| text.contains(fragment))
    }

    fn get_json<T: for<'de> serde::Deserialize<'de>>(
        url: &str,
    ) -> Result<T, Box<dyn std::error::Error>> {
        let response = ureq::get(url).call()?;
        Ok(serde_json::from_str(&response.into_string()?)?)
    }

    fn append_samples(path: &str, passages: Vec<Sample>) -> Result<(), Box<dyn std::error::Error>> {
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        let existing = fs::read_to_string(path)?.lines().count();
        for (index, mut sample) in passages.into_iter().enumerate() {
            sample.id = format!("human-cap-{:06}", existing + index + 1);
            sample.pair_id = Some(format!("pair-cap-{:06}", existing + index + 1));
            writeln!(file, "{}", serde_json::to_string(&sample)?)?;
        }
        Ok(())
    }
}

#[cfg(feature = "sourcing")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    app::run()
}

#[cfg(not(feature = "sourcing"))]
fn main() {}
