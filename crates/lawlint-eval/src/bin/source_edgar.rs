#[cfg(feature = "sourcing")]
mod app {
    use lawlint_eval::sourcing::{segment, strip_html, trim_contract_preamble};
    use lawlint_eval::{Label, Sample};
    use serde::Deserialize;
    use std::fs::{self, OpenOptions};
    use std::io::Write;
    use std::thread::sleep;
    use std::time::Duration;

    const USER_AGENT: &str = "lawlint-eval seth@litvue.com";

    #[derive(Debug, Deserialize)]
    struct SearchResponse {
        hits: Hits,
    }

    #[derive(Debug, Deserialize)]
    struct Hits {
        hits: Vec<Hit>,
    }

    #[derive(Debug, Deserialize)]
    struct Hit {
        #[serde(rename = "_id")]
        id: String,
        #[serde(rename = "_source")]
        source: Source,
    }

    #[derive(Debug, Deserialize)]
    struct Source {
        ciks: Vec<String>,
        adsh: String,
        file_date: String,
        file_type: String,
    }

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let mut samples = Vec::new();
        let mut seen_filings = std::collections::HashSet::new();
        let mut genre_counts = std::collections::HashMap::new();
        for year in (2015..=2019).rev() {
            for query in [
                "credit agreement",
                "loan agreement",
                "lease agreement",
                "license agreement",
                "supply agreement",
                "services agreement",
                "merger agreement",
                "asset purchase agreement",
                "employment agreement",
            ] {
                if samples.len() >= 70 {
                    break;
                }
                let url = format!(
                    "https://efts.sec.gov/LATEST/search-index?q={}&startdt={year}-01-01&enddt={year}-12-31&from=0&size=100",
                    query.replace(' ', "%20")
                );
                let response: SearchResponse = match get_json(&url) {
                    Ok(response) => response,
                    Err(error) => {
                        eprintln!("SEC search failed for {year}/{query}: {error}");
                        continue;
                    }
                };
                for hit in response.hits.hits {
                    if !hit.source.file_type.starts_with("EX-10") || hit.source.ciks.is_empty() {
                        continue;
                    }
                    if !seen_filings.insert(hit.source.adsh.clone()) {
                        continue;
                    }
                    let filename = hit.id.split_once(':').map(|(_, name)| name).unwrap_or("");
                    if filename.is_empty() {
                        continue;
                    }
                    let accession = hit.source.adsh.replace('-', "");
                    let url = format!(
                        "https://www.sec.gov/Archives/edgar/data/{}/{}/{}",
                        hit.source.ciks[0].trim_start_matches('0'),
                        accession,
                        filename
                    );
                    let response = ureq::get(&url).set("User-Agent", USER_AGENT).call()?;
                    let content_type = response.header("Content-Type").unwrap_or_default();
                    if content_type.contains("pdf") {
                        continue;
                    }
                    let html = response.into_string()?;
                    if html.as_bytes().iter().filter(|byte| **byte >= 32).count() * 100
                        < html.len() * 90
                    {
                        continue;
                    }
                    let body = html
                        .split_once("<body")
                        .and_then(|(_, remainder)| remainder.split_once('>').map(|(_, body)| body))
                        .unwrap_or(&html);
                    let cleaned = trim_contract_preamble(&strip_html(body));
                    let genre = contract_genre(&cleaned);
                    let count = genre_counts.entry(genre).or_insert(0);
                    let quota = genre_quota(genre);
                    if *count >= quota {
                        continue;
                    }
                    for text in segment(&cleaned, 100, 500)
                        .into_iter()
                        .filter(|text| text.split_whitespace().count() <= 500)
                        .take(1)
                    {
                        samples.push(Sample {
                            id: format!("human-edgar-{:06}", samples.len() + 1),
                            label: Label::Human,
                            word_count: Some(text.split_whitespace().count()),
                            text,
                            source: "sec-edgar".to_string(),
                            register: "contract".to_string(),
                            era: Some("pre-2020".to_string()),
                            date: Some(hit.source.file_date.clone()),
                            court: None,
                            model: None,
                            prompt_style: None,
                            pair_id: Some(format!("pair-edgar-{:06}", samples.len() + 1)),
                            split: None,
                        });
                        *count += 1;
                    }
                    if samples.len() >= 70 {
                        break;
                    }
                    sleep(Duration::from_millis(200));
                }
                if samples.len() >= 70 {
                    break;
                }
            }
            if samples.len() >= 70 {
                break;
            }
        }
        append_samples("crates/lawlint-eval/corpus/corpus.jsonl", samples)?;
        Ok(())
    }

    fn contract_genre(text: &str) -> &'static str {
        let upper = text.to_ascii_uppercase();
        if upper.contains("CREDIT AGREEMENT")
            || upper.contains("TERM LOAN")
            || upper.contains("REVOLVING LOAN")
        {
            "credit"
        } else if upper.contains("LEASE AGREEMENT") || upper.contains("LANDLORD") {
            "lease"
        } else if upper.contains("LICENSE AGREEMENT") || upper.contains("LICENCE AGREEMENT") {
            "license"
        } else if upper.contains("SUPPLY AGREEMENT")
            || upper.contains("MANUFACTURING AGREEMENT")
            || upper.contains("DISTRIBUTION AGREEMENT")
        {
            "supply"
        } else if upper.contains("MERGER AGREEMENT")
            || upper.contains("ASSET PURCHASE")
            || upper.contains("PURCHASE AGREEMENT")
        {
            "merger"
        } else if upper.contains("SERVICES AGREEMENT")
            || upper.contains("SERVICE AGREEMENT")
            || upper.contains("CONSULTING AGREEMENT")
        {
            "services"
        } else if upper.contains("EMPLOYMENT AGREEMENT")
            || upper.contains("OFFER OF EMPLOYMENT")
            || upper.contains("SEVERANCE AGREEMENT")
        {
            "employment"
        } else {
            "other"
        }
    }

    fn genre_quota(genre: &str) -> usize {
        match genre {
            "employment" => 20,
            "credit" => 12,
            "lease" | "license" | "supply" | "merger" | "services" => 8,
            _ => 0,
        }
    }

    fn get_json<T: for<'de> serde::Deserialize<'de>>(
        url: &str,
    ) -> Result<T, Box<dyn std::error::Error>> {
        let response = ureq::get(url).set("User-Agent", USER_AGENT).call()?;
        Ok(serde_json::from_str(&response.into_string()?)?)
    }

    fn append_samples(
        path: &str,
        mut samples: Vec<Sample>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let existing = fs::read_to_string(path)?.lines().count();
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        for (index, sample) in samples.iter_mut().enumerate() {
            sample.id = format!("human-edgar-{:06}", existing + index + 1);
            sample.pair_id = Some(format!("pair-edgar-{:06}", existing + index + 1));
            writeln!(file, "{}", serde_json::to_string(sample)?)?;
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
