use super::{NmeaBody, NmeaDate, NmeaSentence, NmeaTime};
use crate::format::{Diagnostics, Parsed};

#[derive(Debug, Clone, PartialEq)]
pub struct EpochSnapshot {
    pub time_of_day: Option<NmeaTime>,
    pub date: Option<NmeaDate>,
    pub gga: Option<super::Gga>,
    pub sentence_count: usize,
    pub diagnostics: Diagnostics,
}

impl EpochSnapshot {
    fn empty(date: Option<NmeaDate>) -> Self {
        Self {
            time_of_day: None,
            date,
            gga: None,
            sentence_count: 0,
            diagnostics: Diagnostics::new(),
        }
    }

    pub fn position(&self) -> Option<crate::Wgs84Geodetic> {
        let gga = self.gga.as_ref()?;
        let latitude = gga.latitude?;
        let longitude = gga.longitude?;
        let altitude_msl_m = gga.altitude_msl_m?;
        let geoid_separation_m = gga.geoid_separation_m?;
        crate::Wgs84Geodetic::new(
            latitude.radians(),
            longitude.radians(),
            altitude_msl_m + geoid_separation_m,
        )
        .ok()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NmeaAccumulator {
    current: Option<EpochSnapshot>,
    carried_date: Option<NmeaDate>,
    max_sentences_per_epoch: usize,
    retained: Vec<u8>,
}

impl Default for NmeaAccumulator {
    fn default() -> Self {
        Self::new()
    }
}

impl NmeaAccumulator {
    pub fn new() -> Self {
        Self {
            current: None,
            carried_date: None,
            max_sentences_per_epoch: 256,
            retained: Vec::new(),
        }
    }

    pub fn with_date(date: NmeaDate) -> Self {
        Self {
            carried_date: Some(date),
            ..Self::new()
        }
    }

    pub fn with_max_sentences_per_epoch(mut self, max: usize) -> Self {
        self.max_sentences_per_epoch = max.max(16);
        self
    }

    pub fn push(&mut self, sentence: &NmeaSentence) -> Option<EpochSnapshot> {
        let time = sentence_time(sentence);
        let boundary = match (
            self.current.as_ref().and_then(|epoch| epoch.time_of_day),
            time,
        ) {
            (Some(current), Some(incoming)) => current.key() != incoming.key(),
            _ => false,
        } || self
            .current
            .as_ref()
            .is_some_and(|epoch| epoch.sentence_count >= self.max_sentences_per_epoch);

        let completed = if boundary { self.current.take() } else { None };
        if self.current.is_none() {
            self.current = Some(EpochSnapshot::empty(self.carried_date));
        }
        let epoch = self.current.as_mut().expect("epoch just opened");
        if epoch.time_of_day.is_none() {
            epoch.time_of_day = time;
        }
        epoch.sentence_count += 1;
        match &sentence.body {
            NmeaBody::Gga(gga) => {
                if epoch.gga.is_none() {
                    epoch.gga = Some(gga.clone());
                }
            }
        }
        completed
    }

    pub fn push_bytes(&mut self, chunk: &[u8]) -> NmeaChunkOutput {
        self.retained.extend_from_slice(chunk);
        let mut output = NmeaChunkOutput::default();
        while let Some(pos) = self.retained.iter().position(|&b| b == b'\n' || b == b'\r') {
            let line = self.retained.drain(..pos).collect::<Vec<_>>();
            while self
                .retained
                .first()
                .is_some_and(|b| *b == b'\n' || *b == b'\r')
            {
                self.retained.remove(0);
            }
            push_line(self, &line, &mut output);
        }
        output
    }

    pub fn finish(&mut self) -> Option<EpochSnapshot> {
        self.current.take()
    }

    pub fn retained_len(&self) -> usize {
        self.retained.len()
    }
}

fn sentence_time(sentence: &NmeaSentence) -> Option<NmeaTime> {
    match &sentence.body {
        NmeaBody::Gga(gga) => gga.time,
    }
}

fn push_line(accumulator: &mut NmeaAccumulator, line: &[u8], output: &mut NmeaChunkOutput) {
    if line.is_empty() {
        return;
    }
    let parsed = match std::str::from_utf8(line) {
        Ok(line) => super::parse_sentence(line),
        Err(_) => {
            super::push_error_skip(
                &mut output.diagnostics,
                super::NmeaError::NotFramed {
                    reason: "non-ASCII byte",
                },
            );
            return;
        }
    };
    match parsed {
        Ok(Parsed { value, diagnostics }) => {
            super::merge_diagnostics(&mut output.diagnostics, diagnostics);
            if let Some(snapshot) = accumulator.push(&value) {
                output.snapshots.push(snapshot);
            }
            output.sentences.push(value);
        }
        Err(error) => super::push_error_skip(&mut output.diagnostics, error),
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct NmeaChunkOutput {
    pub snapshots: Vec<EpochSnapshot>,
    pub sentences: Vec<NmeaSentence>,
    pub diagnostics: Diagnostics,
}
