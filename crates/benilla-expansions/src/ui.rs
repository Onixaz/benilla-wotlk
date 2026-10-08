//! How each reference client hands a script handler its frame and arguments.

/// The calling convention of an XML or `SetScript` handler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptArgs {
    /// 1.12.1: an XML body is the chunk itself, taking no arguments (`0x704c70`), and every
    /// handler is called with none (`0x705199`); it reads `this`, `event` and `arg1..argN`.
    Globals,
    /// 3.3.5a: an XML body is compiled as `return function(<params>) <body> end`, the parameter
    /// list chosen by script name ([`ScriptArgs::parameters`]), and every handler is called with
    /// `self` then its arguments. `this` and `event` are still set; no `argN` is (the client has
    /// no such name).
    Parameters,
}

impl ScriptArgs {
    /// The parameter list a 3.3.5a XML body of `script` is compiled under, or `None` under
    /// [`ScriptArgs::Globals`]. Each is one of `Wow.exe`'s `return function(…) %s end` templates
    /// (`0x9eb31c`…`0xaa2ae8`), picked by a name compare ahead of the template's store; any other
    /// name takes the bare `self` (`0x9eb868`). Names are matched as the XML schema spells them.
    pub fn parameters(self, script: &str) -> Option<&'static str> {
        match self {
            ScriptArgs::Globals => None,
            ScriptArgs::Parameters => Some(wotlk_parameters(script)),
        }
    }

    /// Whether the `arg1..argN` globals are set around a call.
    pub fn sets_arg_globals(self) -> bool {
        self == ScriptArgs::Globals
    }
}

fn wotlk_parameters(script: &str) -> &'static str {
    match script {
        "OnEvent" => "self,event,...",
        "OnUpdate" => "self,elapsed",
        "OnClick" | "PreClick" | "PostClick" => "self,button,down",
        "OnMouseDown" | "OnMouseUp" | "OnDragStart" | "OnDoubleClick" => "self,button",
        "OnEnter" | "OnLeave" => "self,motion",
        "OnMouseWheel" => "self,delta",
        "OnKeyDown" | "OnKeyUp" => "self,key",
        "OnChar" | "OnCharComposition" | "OnMovieShowSubtitle" => "self,text",
        "OnSizeChanged" => "self,w,h",
        "OnAttributeChanged" => "self,name,value",
        "OnValueChanged" => "self,value",
        "OnMinMaxChanged" => "self,min,max",
        "OnTextChanged" => "self, userInput",
        "OnInputLanguageChanged" => "self,language",
        "OnCursorChanged" => "self,x,y,w,h",
        "OnVerticalScroll" | "OnHorizontalScroll" => "self,offset",
        "OnScrollRangeChanged" => "self,xrange,yrange",
        "OnColorSelect" => "self,r,g,b",
        "OnHyperlinkClick" => "self,link,text,button",
        "OnHyperlinkEnter" | "OnHyperlinkLeave" => "self,link,text",
        "OnTooltipAddMoney" => "self,cost,maxcost",
        // Animation scripts.
        "OnStop" | "OnFinished" => "self,requested",
        "OnLoop" => "self,loopState",
        _ => "self",
    }
}

#[cfg(test)]
mod tests {
    use super::ScriptArgs;

    #[test]
    fn classic_compiles_no_parameters_and_sets_the_arg_globals() {
        assert_eq!(ScriptArgs::Globals.parameters("OnEvent"), None);
        assert!(ScriptArgs::Globals.sets_arg_globals());
    }

    #[test]
    fn wotlk_picks_each_script_s_template_and_defaults_to_self() {
        let p = |s| ScriptArgs::Parameters.parameters(s).unwrap();
        assert_eq!(p("OnEvent"), "self,event,...");
        assert_eq!(p("OnUpdate"), "self,elapsed");
        assert_eq!(p("OnClick"), "self,button,down");
        assert_eq!(p("OnEnter"), "self,motion");
        assert_eq!(p("OnTextChanged"), "self, userInput");
        assert_eq!(p("OnLoad"), "self");
        assert_eq!(p("OnShow"), "self");
        assert_eq!(p("OnDragStop"), "self");
        assert!(!ScriptArgs::Parameters.sets_arg_globals());
    }
}
