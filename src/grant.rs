//! What an operator signs to let somebody else drive their workspace.
//!
//! The console holds no secret. It holds the public half of the operator's own key, and
//! everything it will accept has to be traceable to that: a grant the operator signed, naming a
//! key and what it may do, and a request signed by that key.
//!
//! A grant on its own is not enough to do anything. Whoever holds one still has to hold the
//! private half of the key it names, so a grant that leaks is a statement about somebody else's
//! permissions and not a way in. That is the reason requests are signed rather than carrying a
//! token: a token is the permission, and a copy of it is the permission again.

use std::collections::BTreeSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// The operator's own key, beside their workspace.
pub const OPERATOR_KEY: &str = "operator.key";

/// How far apart the two clocks may be before a signed request is refused. A replay of a request
/// somebody captured is worth something for as long as this, and a machine whose clock is further
/// out than this cannot drive a console.
pub const WINDOW_SECONDS: i64 = 300;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Grant {
    /// Which workspace this is about, by the name it calls itself.
    pub workspace: String,
    /// Whose key may use it.
    pub to: String,
    /// `read`, `run`, `apply`. Absent is refused rather than assumed.
    pub can: Vec<String>,
    /// A date, after which the console stops taking it.
    pub until: String,
    /// Free text, so somebody reading the file can tell what they agreed to.
    #[serde(default)]
    pub why: String,
}

impl Grant {
    /// The bytes that are signed: the statement, written once, so the signature is over what a
    /// reader of the file sees rather than over a re-serialisation of it.
    pub fn statement(&self) -> String {
        format!(
            "zetlyn-grant-2\nworkspace={}\nto={}\ncan={}\nuntil={}\n",
            self.workspace,
            self.to.trim(),
            self.can.join(","),
            self.until
        )
    }

    pub fn allows(&self, what: &str) -> bool {
        self.can.iter().any(|c| c == what)
    }

    /// Whether this grant is still one today, and covers this workspace and this act.
    pub fn holds(&self, workspace: &str, what: &str, today: &str) -> Result<(), String> {
        if self.workspace != workspace {
            return Err(format!(
                "that grant is for {}, and this is {workspace}",
                self.workspace
            ));
        }
        if self.until.as_str() < today {
            return Err(format!("that grant ran out on {}", self.until));
        }
        if !self.allows(what) {
            return Err(format!(
                "that grant may {} and not {what}",
                self.can.join(", ")
            ));
        }
        Ok(())
    }
}

/// A grant and the operator's signature over it, in one file, because the two are useless apart.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signed {
    #[serde(flatten)]
    pub grant: Grant,
    pub signature: String,
}

impl Signed {
    pub fn write(&self, path: &Path) -> Result<(), String> {
        let text = crate::yaml::to_string(self)?;
        std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Signed by the operator of this workspace, and not by anybody else.
    pub fn by(&self, operator: &str) -> Result<(), String> {
        crate::key::verify(operator, self.grant.statement().as_bytes(), &self.signature)
            .map_err(|e| format!("the grant is not this operator's: {e}"))
    }
}

/// Write one, signed with the operator's key. The key is made on the first grant, because that is
/// the first moment there is anything to sign.
pub fn issue(
    workspace: &Path,
    name: &str,
    to: &str,
    can: &[String],
    until: &str,
    why: &str,
) -> Result<Signed, String> {
    crate::key::bytes(to).map_err(|e| format!("--to {to}: {e}"))?;
    let known: BTreeSet<&str> = ["read", "run", "apply"].into_iter().collect();
    for one in can {
        if !known.contains(one.as_str()) {
            return Err(format!(
                "{one}: a grant may read, run or apply, and nothing else"
            ));
        }
    }
    if can.is_empty() {
        return Err("--can what? read, run, apply".into());
    }
    if until.len() != 10 || *until <= *crate::iso_date(crate::now()) {
        return Err(format!("--until {until}: a date, later than today"));
    }
    if crate::key::public(workspace, OPERATOR_KEY).is_none() {
        let public = crate::key::new(workspace, OPERATOR_KEY)?;
        println!("this workspace is {public}");
    }
    let grant = Grant {
        workspace: name.to_string(),
        to: to.trim().to_string(),
        can: can.to_vec(),
        until: until.to_string(),
        why: why.to_string(),
    };
    let signature = crate::key::sign(workspace, OPERATOR_KEY, grant.statement().as_bytes())?
        .ok_or("this workspace has no operator key")?;
    Ok(Signed { grant, signature })
}

// -- a signed request ---------------------------------------------------------------------------

/// What a caller signs. The method and the path so a grant to read cannot be replayed as a write,
/// the body so it cannot be swapped, and the time so a captured request stops working.
pub fn request_statement(method: &str, path: &str, body: &[u8], at: &str) -> String {
    format!(
        "zetlyn-call-1\n{method}\n{path}\n{}\n{at}\n",
        crate::place::sha256(body)
    )
}

/// Held against the key the grant names, and against the clock.
pub fn check_request(
    grant: &Grant,
    method: &str,
    path: &str,
    body: &[u8],
    at: &str,
    signature: &str,
) -> Result<(), String> {
    let now = crate::now();
    let then = crate::fetch::seconds_of(at);
    if then == 0 {
        return Err("that call carries no time".into());
    }
    if (now - then).abs() > WINDOW_SECONDS {
        return Err(format!(
            "that call is stamped {at}, which is more than {WINDOW_SECONDS} seconds from now"
        ));
    }
    crate::key::verify(
        &grant.to,
        request_statement(method, path, body, at).as_bytes(),
        signature,
    )
    .map_err(|e| format!("the call is not signed by the key the grant names: {e}"))
}
