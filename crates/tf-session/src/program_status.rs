use tf_tap::{ProgramState, ProgramStatusReport};

const MAX_RECORDS: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramStatusRecord {
    pub report: ProgramStatusReport,
    touched: u64,
}

#[derive(Debug, Default)]
pub struct ProgramStatusIndex {
    records: Vec<ProgramStatusRecord>,
    clock: u64,
}

impl ProgramStatusIndex {
    pub fn apply(&mut self, report: ProgramStatusReport) {
        let id = report.id.as_deref().unwrap_or_default();
        if report.state == ProgramState::Clear {
            self.records.retain(|record| {
                let candidate = record.report.id.as_deref().unwrap_or_default();
                if id.is_empty() {
                    false
                } else {
                    candidate != id && !candidate.starts_with(&format!("{id}/"))
                }
            });
            return;
        }

        self.clock = self.clock.wrapping_add(1);
        if let Some(existing) = self
            .records
            .iter_mut()
            .find(|record| record.report.id.as_deref().unwrap_or_default() == id)
        {
            existing.report = report;
            existing.touched = self.clock;
            return;
        }
        if self.records.len() == MAX_RECORDS {
            let oldest = self
                .records
                .iter()
                .enumerate()
                .min_by_key(|(_, record)| record.touched)
                .map(|(index, _)| index)
                .unwrap_or_default();
            self.records.remove(oldest);
        }
        self.records.push(ProgramStatusRecord {
            report,
            touched: self.clock,
        });
    }

    pub fn clear_transient(&mut self) -> bool {
        let before = self.records.len();
        self.records.retain(|record| {
            !matches!(
                record.report.state,
                ProgramState::Working | ProgramState::Blocked
            )
        });
        before != self.records.len()
    }

    pub fn clear(&mut self) -> bool {
        let changed = !self.records.is_empty();
        self.records.clear();
        changed
    }

    pub fn replace(&mut self, reports: impl IntoIterator<Item = ProgramStatusReport>) {
        self.records.clear();
        for report in reports {
            self.apply(report);
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &ProgramStatusRecord> {
        self.records.iter()
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}
