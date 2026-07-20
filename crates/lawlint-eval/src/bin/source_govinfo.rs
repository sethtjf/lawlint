#[cfg(feature = "sourcing")]
mod app {
    use lawlint_eval::sourcing::{segment, strip_html};
    use lawlint_eval::{Label, Sample};
    use std::fs::{self, OpenOptions};
    use std::io::Write;
    use std::thread::sleep;
    use std::time::Duration;

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let urls = [
            (
                "statute",
                "USCODE-2018-title18/html/USCODE-2018-title18-partI-chap1-sec1.htm",
            ),
            (
                "statute",
                "USCODE-2018-title18/html/USCODE-2018-title18-partI-chap2-sec111.htm",
            ),
            (
                "statute",
                "USCODE-2018-title18/html/USCODE-2018-title18-part2-chap101-sec1341.htm",
            ),
            (
                "statute",
                "USCODE-2018-title18/html/USCODE-2018-title18-part2-chap102-sec1343.htm",
            ),
            (
                "statute",
                "USCODE-2018-title18/html/USCODE-2018-title18-part2-chap109-sec1951.htm",
            ),
            (
                "statute",
                "USCODE-2018-title26/html/USCODE-2018-title26-subtitleA-chap1-subchapA-sec1.htm",
            ),
            (
                "statute",
                "USCODE-2018-title26/html/USCODE-2018-title26-subtitleA-chap1-subchapC-sec61.htm",
            ),
            (
                "statute",
                "USCODE-2018-title26/html/USCODE-2018-title26-subtitleA-chap1-subchapD-sec101.htm",
            ),
            (
                "statute",
                "USCODE-2018-title29/html/USCODE-2018-title29-chap7-subchapII-sec151.htm",
            ),
            (
                "statute",
                "USCODE-2018-title29/html/USCODE-2018-title29-chap7-subchapII-sec157.htm",
            ),
            (
                "statute",
                "USCODE-2018-title29/html/USCODE-2018-title29-chap7-subchapII-sec158.htm",
            ),
            (
                "statute",
                "USCODE-2018-title29/html/USCODE-2018-title29-chap8-sec201.htm",
            ),
            (
                "statute",
                "USCODE-2018-title42/html/USCODE-2018-title42-chap7-subchapXIX-sec1396.htm",
            ),
            (
                "statute",
                "USCODE-2018-title42/html/USCODE-2018-title42-chap85-sec7401.htm",
            ),
            (
                "statute",
                "USCODE-2018-title47/html/USCODE-2018-title47-chap5-subchapII-sec201.htm",
            ),
            (
                "statute",
                "USCODE-2018-title16/html/USCODE-2018-title16-chap1-sec1.htm",
            ),
            (
                "statute",
                "USCODE-2018-title21/html/USCODE-2018-title21-chap9-subchapII-sec301.htm",
            ),
            (
                "statute",
                "USCODE-2018-title35/html/USCODE-2018-title35-partII-chap17-sec101.htm",
            ),
            (
                "statute",
                "USCODE-2018-title40/html/USCODE-2018-title40-subtitleI-sec101.htm",
            ),
            (
                "statute",
                "USCODE-2018-title50/html/USCODE-2018-title50-chap36-sec1801.htm",
            ),
            (
                "statute",
                "USCODE-2018-title15/html/USCODE-2018-title15-chap1-sec1.htm",
            ),
            (
                "statute",
                "USCODE-2018-title17/html/USCODE-2018-title17-chap1-sec101.htm",
            ),
            (
                "statute",
                "USCODE-2018-title31/html/USCODE-2018-title31-subtitleIII-sec5311.htm",
            ),
            (
                "statute",
                "USCODE-2018-title8/html/USCODE-2018-title8-chap12-subchapI-sec1101.htm",
            ),
            (
                "statute",
                "USCODE-2018-title28/html/USCODE-2018-title28-partIV-chap85-sec1291.htm",
            ),
            (
                "statute",
                "USCODE-2018-title18/html/USCODE-2018-title18-partI-chap51-sec1030.htm",
            ),
            (
                "statute",
                "USCODE-2018-title18/html/USCODE-2018-title18-partII-chap96-sec1961.htm",
            ),
            (
                "statute",
                "USCODE-2018-title18/html/USCODE-2018-title18-partII-chap96-sec1962.htm",
            ),
            (
                "statute",
                "USCODE-2018-title18/html/USCODE-2018-title18-partII-chap113B-sec2339B.htm",
            ),
            (
                "statute",
                "USCODE-2018-title26/html/USCODE-2018-title26-subtitleA-chap1-subchapB-sec162.htm",
            ),
            (
                "statute",
                "USCODE-2018-title42/html/USCODE-2018-title42-chap21-sec1983.htm",
            ),
            (
                "statute",
                "USCODE-2018-title15/html/USCODE-2018-title15-chap2B-sec78j.htm",
            ),
            (
                "statute",
                "USCODE-2018-title17/html/USCODE-2018-title17-chap1-sec106.htm",
            ),
            (
                "statute",
                "USCODE-2018-title8/html/USCODE-2018-title8-chap12-subchapII-sec1324.htm",
            ),
            (
                "statute",
                "USCODE-2018-title18/html/USCODE-2018-title18-partII-chap96-sec1964.htm",
            ),
            (
                "statute",
                "USCODE-2018-title18/html/USCODE-2018-title18-partII-chap96-sec1968.htm",
            ),
            (
                "statute",
                "USCODE-2018-title42/html/USCODE-2018-title42-chap21-sec2000e.htm",
            ),
            (
                "statute",
                "USCODE-2018-title29/html/USCODE-2018-title29-chap7-subchapII-sec621.htm",
            ),
            (
                "statute",
                "USCODE-2018-title26/html/USCODE-2018-title26-subtitleA-chap1-subchapB-sec280A.htm",
            ),
            (
                "statute",
                "USCODE-2018-title15/html/USCODE-2018-title15-chap41-sec1692.htm",
            ),
            (
                "statute",
                "USCODE-2018-title35/html/USCODE-2018-title35-partIII-chap27-sec271.htm",
            ),
            ("regulation", "CFR-2019-title17-vol4-sec240-10b-5.htm"),
            ("regulation", "CFR-2019-title29-vol4-sec1910-1200.htm"),
            ("regulation", "CFR-2019-title40-vol30-sec261-4.htm"),
            ("regulation", "CFR-2019-title45-vol1-sec46-111.htm"),
            ("regulation", "CFR-2019-title47-vol4-sec64-1200.htm"),
        ];
        let mut samples = Vec::new();
        for (register, path) in urls {
            let url = format!("https://www.govinfo.gov/content/pkg/{path}");
            let html = match ureq::get(&url).call() {
                Ok(response) => response.into_string()?,
                Err(error) => {
                    eprintln!("govinfo fetch failed for {path}: {error}");
                    continue;
                }
            };
            let statute = html
                .split_once("<!-- field-start:statute -->")
                .and_then(|(_, remainder)| remainder.split_once("<!-- field-end:statute -->"))
                .map_or("", |(statute, _)| statute);
            for text in segment(&strip_html(statute), 100, 500)
                .into_iter()
                .filter(|text| text.split_whitespace().count() <= 500)
                .take(2)
            {
                samples.push(Sample {
                    id: format!("human-govinfo-{:06}", samples.len() + 1),
                    label: Label::Human,
                    word_count: Some(text.split_whitespace().count()),
                    text,
                    source: "govinfo".to_string(),
                    register: register.to_string(),
                    era: Some("pre-2020".to_string()),
                    date: Some("2018".to_string()),
                    court: None,
                    model: None,
                    prompt_style: None,
                    pair_id: Some(format!("pair-govinfo-{:06}", samples.len() + 1)),
                    split: None,
                });
            }
            sleep(Duration::from_millis(150));
        }
        append_samples("crates/lawlint-eval/corpus/corpus.jsonl", samples)?;
        Ok(())
    }

    fn append_samples(
        path: &str,
        mut samples: Vec<Sample>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let existing = fs::read_to_string(path)?.lines().count();
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        for (index, sample) in samples.iter_mut().enumerate() {
            sample.id = format!("human-govinfo-{:06}", existing + index + 1);
            sample.pair_id = Some(format!("pair-govinfo-{:06}", existing + index + 1));
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
