//! The GM ticket globals the stock Help window calls (`0x5ad070`-`0x5ad230`). The verbs share one
//! ordered queue, because call order is the reference's wire order, and two calls are two sends:
//! the ticket toast re-polls `GetGMTicket()` every 10 minutes. `GetGMTicket()` is a send, answered
//! by the `UPDATE_TICKET` event; the client's ticket and response ids live in the app, which gates
//! each write on them as the senders do.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{ftol, number_arg};

/// One ticket verb the window called, drained in call order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GmTicketIntent {
    /// `GetGMTicket()` (`0x5ad070`): `CMSG_GMTICKET_GETTICKET`.
    Ask,
    /// `GetGMStatus()` (`0x5ad1c0`): `CMSG_GMTICKET_SYSTEMSTATUS`.
    AskStatus,
    /// `DeleteGMTicket()` (`0x5ad140`): `CMSG_GMTICKET_DELETETICKET`.
    Delete,
    /// `NewGMTicket(text, needResponse)` (`0x5ad080`), or `GMResponseNeedMoreHelp(text)`
    /// (`0x5ad150`), which files the follow-up with a reply wanted: `CMSG_GMTICKET_CREATE`.
    Create {
        text: String,
        need_response: bool,
        more_help: bool,
    },
    /// `UpdateGMTicket(text)` (`0x5ad0f0`): `CMSG_GMTICKET_UPDATETEXT`.
    Update { text: String },
    /// `GMResponseResolve()` (`0x5ad1b0`): `CMSG_GMRESPONSE_RESOLVE`.
    Resolve,
    /// `GMReportLag(lagType)` (`0x5ad020`): `CMSG_GM_REPORT_LAG` with the argument less one,
    /// wrapping, and where the player stands; nothing without a player (`0x5acf30`).
    ReportLag { kind: u32 },
    /// `GMSurveySubmit()` (`0x5acd50`): `CMSG_GMSURVEY_SUBMIT`, ungated.
    SurveySubmit(SurveySubmission),
}

/// The survey slots per survey (`GMSurveySurveys`' ten question columns).
pub const SURVEY_QUESTIONS: usize = 10;

/// The survey's tables, as `GMSurveyQuestion` and `GMSurveyAnswer` read them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SurveyTables {
    /// `GMSurveyCurrentSurvey`'s survey for the locale, 0 without one.
    pub id: u32,
    /// That survey's question ids, 0 for an empty slot.
    pub questions: [u32; SURVEY_QUESTIONS],
    /// `(question id, text)` per `GMSurveyQuestions` row.
    pub question_text: Vec<(u32, String)>,
    /// `(question id, sort index, text)` per `GMSurveyAnswers` row, in file order.
    pub answers: Vec<(u32, u32, String)>,
}

/// The pushed tables and what the window answered since the last submit: the ranks
/// (`0xc1e8c4`), each question's comment (`0xc1e8d8`) and the overall comment (`0xc1e900`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SurveyState {
    pub(crate) tables: SurveyTables,
    ranks: [u8; SURVEY_QUESTIONS],
    comments: [Option<String>; SURVEY_QUESTIONS],
    comment: Option<String>,
}

/// One `CMSG_GMSURVEY_SUBMIT` body's content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurveySubmission {
    pub survey: u32,
    /// `(question id, rank, comment)` per slot, up to the first empty question id.
    pub answers: Vec<(u32, u8, String)>,
    pub comment: String,
}

impl SurveyState {
    /// The question id in 1-based slot `index`, when the survey names one there with a row.
    fn question(&self, index: i32) -> Option<u32> {
        let slot = usize::try_from(index.checked_sub(1)?).ok()?;
        let qid = *self.tables.questions.get(slot)?;
        (qid > 0 && self.tables.question_text.iter().any(|(id, _)| *id == qid)).then_some(qid)
    }

    /// Build the submission, then forget every answer (`0x5ac1f0`).
    fn submit(&mut self) -> SurveySubmission {
        let mut answers = Vec::new();
        for (i, &qid) in self.tables.questions.iter().enumerate() {
            if qid == 0 {
                break;
            }
            let comment = self.comments[i].take().unwrap_or_default();
            answers.push((qid, self.ranks[i], comment));
        }
        let out = SurveySubmission {
            survey: self.tables.id,
            answers,
            comment: self.comment.take().unwrap_or_default(),
        };
        self.ranks = [0; SURVEY_QUESTIONS];
        self.comments = Default::default();
        out
    }
}

impl super::UiScript {
    /// The `GMTicketCategory.dbc` rows in file order, pushed once.
    pub fn set_gm_ticket_categories(&mut self, categories: Vec<(u32, String)>) {
        set_field!(self, gm_ticket_categories, categories);
    }

    /// The survey tables, pushed once.
    pub fn set_gm_survey(&mut self, tables: SurveyTables) {
        set_field!(self, gm_survey.tables, tables);
    }

    /// Drain the ticket verbs in call order, one packet each.
    pub fn take_gm_ticket_intents(&mut self) -> Vec<GmTicketIntent> {
        drain_field!(self, gm_ticket_intents)
    }

    /// Drain the `Stuck()` calls, each one cast of spell 7355.
    pub fn take_stuck_casts(&mut self) -> u32 {
        drain_field!(self, stuck_casts)
    }
}

/// `lua_isstring` then `lua_tolstring`: a string, or a number in its string form.
fn ticket_text(lua: &Lua, v: Option<&Value>) -> mlua::Result<Option<String>> {
    match v {
        Some(v @ (Value::String(_) | Value::Integer(_) | Value::Number(_))) => {
            Ok(lua.coerce_string(v.clone())?.map(|s| s.to_string_lossy()))
        }
        _ => Ok(None),
    }
}

fn push(lua: &Lua, intent: GmTicketIntent) {
    crate::script::model_mut_of(lua)
        .gm_ticket_intents
        .push(intent);
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // A flat `id, name, ...` vararg list; empty until the app pushes the DBC.
    g.set(
        "GetGMTicketCategories",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let mut out = Vec::with_capacity(model.gm_ticket_categories.len() * 2);
            for (id, name) in &model.gm_ticket_categories {
                out.push(Value::Integer(i64::from(*id)));
                out.push(Value::String(lua.create_string(name)?));
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // The argument-free sends.
    for (name, intent) in [
        ("GetGMTicket", GmTicketIntent::Ask),
        ("GetGMStatus", GmTicketIntent::AskStatus),
        ("DeleteGMTicket", GmTicketIntent::Delete),
        ("GMResponseResolve", GmTicketIntent::Resolve),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, ()| {
                push(lua, intent.clone());
                Ok(())
            })?,
        )?;
    }

    // NewGMTicket(ticketDescription, needResponse): a string and a real boolean (`0x5ad0a1`),
    // else the usage error.
    g.set(
        "NewGMTicket",
        lua.create_function(|lua, args: MultiValue| {
            let text = ticket_text(lua, args.front())?;
            let need = args.get(1);
            let (Some(text), Some(Value::Boolean(need_response))) = (text, need) else {
                return Err(mlua::Error::runtime(
                    "Usage: NewGMTicket(ticketDescription, needResponse)",
                ));
            };
            push(
                lua,
                GmTicketIntent::Create {
                    text,
                    need_response: *need_response,
                    more_help: false,
                },
            );
            Ok(())
        })?,
    )?;

    // UpdateGMTicket(ticketDescription).
    g.set(
        "UpdateGMTicket",
        lua.create_function(|lua, args: MultiValue| {
            let Some(text) = ticket_text(lua, args.front())? else {
                return Err(mlua::Error::runtime(
                    "Usage: UpdateGMTicket(ticketDescription)",
                ));
            };
            push(lua, GmTicketIntent::Update { text });
            Ok(())
        })?,
    )?;

    // GMResponseNeedMoreHelp(newTicketDescription): a create with a reply wanted and the
    // need-more-help byte set (`0x5ad191`).
    g.set(
        "GMResponseNeedMoreHelp",
        lua.create_function(|lua, args: MultiValue| {
            let Some(text) = ticket_text(lua, args.front())? else {
                return Err(mlua::Error::runtime(
                    "Usage: GMResponseNeedMoreHelp(newTicketDescription)",
                ));
            };
            push(
                lua,
                GmTicketIntent::Create {
                    text,
                    need_response: true,
                    more_help: true,
                },
            );
            Ok(())
        })?,
    )?;

    // The Help window's Auto-Unstuck, not a ticket verb: it casts spell 7355, whose
    // `SPELL_EFFECT_STUCK` moves the player to a safe position server-side.
    g.set(
        "Stuck",
        lua.create_function(|lua, ()| {
            crate::script::model_mut_of(lua).stuck_casts += 1;
            Ok(())
        })?,
    )?;

    // GMReportLag(lagType) (`0x5ad020`).
    g.set(
        "GMReportLag",
        lua.create_function(|lua, kind: Value| {
            let kind = number_arg(lua, kind, "Usage: GMReportLag(number)")?;
            push(
                lua,
                GmTicketIntent::ReportLag {
                    kind: (kind as u32).wrapping_sub(1),
                },
            );
            Ok(())
        })?,
    )?;

    // GMSurveyQuestion(index) (`0x5ac750`): the question text in slot `index`, else nil.
    g.set(
        "GMSurveyQuestion",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: GMSurveyGetQuestion(index)")?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let survey = &model.gm_survey;
            Ok(survey.question(index).and_then(|qid| {
                survey
                    .tables
                    .question_text
                    .iter()
                    .find(|(id, _)| *id == qid)
                    .map(|(_, t)| t.clone())
            }))
        })?,
    )?;

    // GMSurveyAnswer(questionIndex, answerIndex) (`0x5ac7d0`): the first answer row in file order
    // naming that question at sort index `answerIndex - 1`, else nil. The usage error needs both
    // arguments to fail the number test; one failing alone reads as 0.
    g.set(
        "GMSurveyAnswer",
        lua.create_function(|lua, (q, a): (Value, Value)| {
            let q = lua.coerce_number(q)?;
            let a = lua.coerce_number(a)?;
            if q.is_none() && a.is_none() {
                return Err(mlua::Error::runtime(
                    "Usage: GMSurveyGetAnswer(questionIndex, answerIndex)",
                ));
            }
            let sort = (ftol(a.unwrap_or(0.0)) as u32).wrapping_sub(1);
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let survey = &model.gm_survey;
            Ok(survey.question(ftol(q.unwrap_or(0.0))).and_then(|qid| {
                survey
                    .tables
                    .answers
                    .iter()
                    .find(|(id, s, _)| *id == qid && *s == sort)
                    .map(|(_, _, t)| t.clone())
            }))
        })?,
    )?;

    // GMSurveyAnswerSubmit(question, rank, comment) (`0x5ac390`): keeps the rank's low byte and
    // the comment, an empty one as none (`0x5ac260`).
    g.set(
        "GMSurveyAnswerSubmit",
        lua.create_function(|lua, args: MultiValue| {
            let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Nil);
            let q = lua.coerce_number(arg(0))?;
            let rank = lua.coerce_number(arg(1))?;
            let comment = ticket_text(lua, args.get(2))?;
            let (Some(q), Some(rank), Some(comment)) = (q, rank, comment) else {
                return Err(mlua::Error::runtime(
                    "Usage: GMSurveyAnswerSubmit(question, rank, comment)",
                ));
            };
            let slot = (ftol(q) as u32).wrapping_sub(1) as usize;
            if slot >= SURVEY_QUESTIONS {
                return Err(mlua::Error::runtime(format!(
                    "GMSurveyAnswerSubmit: Questions limited from 1 to {SURVEY_QUESTIONS}"
                )));
            }
            let mut model = crate::script::model_mut_of(lua);
            let survey = &mut model.gm_survey;
            survey.ranks[slot] = ftol(rank) as u8;
            survey.comments[slot] = (!comment.is_empty()).then_some(comment);
            Ok(())
        })?,
    )?;

    // GMSurveyCommentSubmit(comment) (`0x5ac480`): replaces the overall comment (`0x5ac2d0`).
    g.set(
        "GMSurveyCommentSubmit",
        lua.create_function(|lua, args: MultiValue| {
            let Some(comment) = ticket_text(lua, args.front())? else {
                return Err(mlua::Error::runtime(
                    "Usage: GMSurveyCommentSubmit(comment)",
                ));
            };
            crate::script::model_mut_of(lua).gm_survey.comment =
                (!comment.is_empty()).then_some(comment);
            Ok(())
        })?,
    )?;

    // GMSurveySubmit() (`0x5ad230`): sends what was answered, then forgets it.
    g.set(
        "GMSurveySubmit",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            let submission = model.gm_survey.submit();
            model
                .gm_ticket_intents
                .push(GmTicketIntent::SurveySubmit(submission));
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    fn survey() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_gm_survey(SurveyTables {
            id: 7,
            questions: [30, 31, 0, 0, 0, 0, 0, 0, 0, 0],
            question_text: vec![(30, "Speed?".into()), (31, "Polite?".into())],
            answers: vec![
                (30, 1, "Slow".into()),
                (30, 0, "Fast".into()),
                (31, 0, "Yes".into()),
            ],
        });
        s
    }

    /// `0x5ac530`/`0x5ac5b0`: a slot reads through the survey's question id; an answer is
    /// matched by its sort index, not its row order.
    #[test]
    fn questions_and_answers_read_through_the_current_survey() {
        let s = survey();
        let q: Option<String> = s.eval("return GMSurveyQuestion(2)").unwrap();
        assert_eq!(q.as_deref(), Some("Polite?"));
        let q: Option<String> = s.eval("return GMSurveyQuestion(3)").unwrap();
        assert_eq!(q, None, "an empty slot");
        let q: Option<String> = s.eval("return GMSurveyQuestion(11)").unwrap();
        assert_eq!(q, None, "past the ten slots");
        let a: Option<String> = s.eval("return GMSurveyAnswer(1, 1)").unwrap();
        assert_eq!(a.as_deref(), Some("Fast"));
        let a: Option<String> = s.eval("return GMSurveyAnswer(1, 'x')").unwrap();
        assert_eq!(a, None, "one non-number reads as 0");
        assert!(s.eval::<()>("GMSurveyAnswer()").is_err());
        assert!(s.eval::<()>("GMSurveyQuestion()").is_err());
    }

    /// `0x5acd50`: the body runs to the first empty question slot, then the answers reset.
    #[test]
    fn a_submit_carries_the_answers_and_forgets_them() {
        let mut s = survey();
        s.run(
            "GMSurveyAnswerSubmit(1, 260, 'quick') GMSurveyAnswerSubmit(2, 3, '') \
             GMSurveyCommentSubmit('thanks') GMSurveySubmit() GMSurveySubmit()",
        )
        .unwrap();
        let intents = s.take_gm_ticket_intents();
        assert_eq!(
            intents,
            vec![
                GmTicketIntent::SurveySubmit(SurveySubmission {
                    survey: 7,
                    answers: vec![(30, 4, "quick".into()), (31, 3, String::new())],
                    comment: "thanks".into(),
                }),
                GmTicketIntent::SurveySubmit(SurveySubmission {
                    survey: 7,
                    answers: vec![(30, 0, String::new()), (31, 0, String::new())],
                    comment: String::new(),
                }),
            ]
        );
        let err = s.run("GMSurveyAnswerSubmit(11, 1, '')").unwrap_err();
        assert!(err.to_string().contains("Questions limited from 1 to 10"));
    }

    /// `0x5ad020`: the lag type goes out less one, wrapping.
    #[test]
    fn report_lag_sends_the_type_less_one() {
        let mut s = UiScript::new().unwrap();
        s.run("GMReportLag(3) GMReportLag(0)").unwrap();
        assert_eq!(
            s.take_gm_ticket_intents(),
            vec![
                GmTicketIntent::ReportLag { kind: 2 },
                GmTicketIntent::ReportLag { kind: u32::MAX },
            ]
        );
        assert!(s.run("GMReportLag('x')").is_err());
    }
}
