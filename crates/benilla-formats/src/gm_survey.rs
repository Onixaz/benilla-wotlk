//! The four `GMSurvey*.dbc` tables behind the post-ticket survey window. 12340 takes no survey id
//! from the server: the client's locale index keys `GMSurveyCurrentSurvey` (`LANGID` →
//! `GMSURVEY_ID`), that row of `GMSurveySurveys` lists ten question ids, and each question's
//! answers are the `GMSurveyAnswers` rows naming it, matched by sort index in file order
//! (`0x5ac530`, `0x5ac5b0`).

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::Chain;

const CURRENT_SURVEY: &str = "DBFilesClient\\GMSurveyCurrentSurvey.dbc";
const SURVEYS: &str = "DBFilesClient\\GMSurveySurveys.dbc";
const QUESTIONS: &str = "DBFilesClient\\GMSurveyQuestions.dbc";
const ANSWERS: &str = "DBFilesClient\\GMSurveyAnswers.dbc";

/// The enUS locale index, the `GMSurveyCurrentSurvey` key benilla's enUS string slot matches.
const LANG_ENUS: u32 = 0;

/// The survey questions per survey (`GMSurveySurveys`' `Q[10]`).
pub const SURVEY_QUESTIONS: usize = 10;

/// The current survey, its questions and every answer row.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GmSurvey {
    /// `GMSurveyCurrentSurvey`'s survey for the locale, 0 without a row.
    pub id: u32,
    /// That survey's ten question ids, 0 for an empty slot; all 0 without a survey row.
    pub questions: [u32; SURVEY_QUESTIONS],
    /// `(question id, text)` for every `GMSurveyQuestions` row.
    pub question_text: Vec<(u32, String)>,
    /// `(question id, sort index, text)` for every `GMSurveyAnswers` row, in file order.
    pub answers: Vec<(u32, u32, String)>,
}

pub(crate) fn current_survey_schema() -> Schema {
    let mut s = Schema::new("GMSurveyCurrentSurvey");
    s.add_field(SchemaField::new("LANGID", FieldType::UInt32));
    s.add_field(SchemaField::new("GMSURVEY_ID", FieldType::UInt32));
    s
}

pub(crate) fn surveys_schema() -> Schema {
    let mut s = Schema::new("GMSurveySurveys");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    for i in 0..SURVEY_QUESTIONS {
        s.add_field(SchemaField::new(format!("Q{i}"), FieldType::UInt32));
    }
    s
}

pub(crate) fn questions_schema() -> Schema {
    let mut s = Schema::new("GMSurveyQuestions");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    for i in 0..16 {
        s.add_field(SchemaField::new(format!("Question{i}"), FieldType::String));
    }
    s.add_field(SchemaField::new("QuestionFlags", FieldType::UInt32));
    s
}

pub(crate) fn answers_schema() -> Schema {
    let mut s = Schema::new("GMSurveyAnswers");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("Sort_index", FieldType::UInt32));
    s.add_field(SchemaField::new("GMSurveyQuestionID", FieldType::UInt32));
    for i in 0..16 {
        s.add_field(SchemaField::new(format!("Answer{i}"), FieldType::String));
    }
    s.add_field(SchemaField::new("AnswerFlags", FieldType::UInt32));
    s
}

/// Load the four tables from the patch chain into the current survey.
pub fn load_gm_survey(chain: &mut Chain) -> Result<GmSurvey> {
    let mut read = |path: &str, schema: Schema, name: &str| {
        let bytes = chain
            .read_file(path)
            .with_context(|| format!("reading {path}"))?;
        parse(&bytes, schema, name)
    };
    let current = read(
        CURRENT_SURVEY,
        current_survey_schema(),
        "GMSurveyCurrentSurvey",
    )?;
    let surveys = read(SURVEYS, surveys_schema(), "GMSurveySurveys")?;
    let questions = read(QUESTIONS, questions_schema(), "GMSurveyQuestions")?;
    let answers = read(ANSWERS, answers_schema(), "GMSurveyAnswers")?;

    let id = current
        .records()
        .iter()
        .find(|r| u32_at(r, 0) == Some(LANG_ENUS))
        .and_then(|r| u32_at(r, 1))
        .unwrap_or(0);
    let mut survey = GmSurvey {
        id,
        ..GmSurvey::default()
    };
    if let Some(r) = surveys.records().iter().find(|r| u32_at(r, 0) == Some(id)) {
        for (i, q) in survey.questions.iter_mut().enumerate() {
            *q = u32_at(r, 1 + i).unwrap_or(0);
        }
    }
    for r in questions.records() {
        if let Some(qid) = u32_at(r, 0) {
            let text = str_at(&questions, r, 1).unwrap_or_default();
            survey.question_text.push((qid, text));
        }
    }
    for r in answers.records() {
        let (Some(sort), Some(qid)) = (u32_at(r, 1), u32_at(r, 2)) else {
            continue;
        };
        let text = str_at(&answers, r, 3).unwrap_or_default();
        survey.answers.push((qid, sort, text));
    }
    Ok(survey)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The enUS survey has questions, each with its answers.
    #[test]
    fn the_shipped_survey_has_questions_with_answers() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let survey = load_gm_survey(&mut chain).expect("GMSurvey tables");
        assert_ne!(survey.id, 0, "enUS has a current survey");
        let first = survey.questions[0];
        assert_ne!(first, 0, "the survey's first slot names a question");
        assert!(survey
            .question_text
            .iter()
            .any(|(id, text)| *id == first && !text.is_empty()));
        assert!(survey.answers.iter().any(|(q, _, _)| *q == first));
    }
}
