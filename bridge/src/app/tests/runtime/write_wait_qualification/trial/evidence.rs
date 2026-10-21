//! Off-source split of the frozen coordinator; no execution evidence.
use super::*;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum WitnessScalar {
    Null,
    Bool(bool),
    I64(i64),
    U64(u64),
    Text(String),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OracleAssertion {
    pub field: String,
    pub actual: WitnessScalar,
    pub expected: WitnessScalar,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum OracleWitness {
    Page {
        oracle: String,
        returned_items: u64,
        total_items: Option<u64>,
        first_key: Option<String>,
        last_key: Option<String>,
        cursor: Option<String>,
        version: Option<String>,
    },
    File {
        oracle: String,
        path_suffix: String,
        bytes: u64,
        sha256: String,
    },
    State {
        oracle: String,
        assertions: Vec<OracleAssertion>,
    },
}
const MAX_WITNESS_RESERIALIZED_BYTES: u64 = 4_096;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EffectProof {
    pub boundary: String,
    pub witness: OracleWitness,
    pub checked_at_us: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResponseEvidence {
    // These describe reserialized DECODED JSON, not encrypted wire bytes or
    // the exact original plaintext whitespace/key-order representation.
    pub decoded_json_reserialized_bytes: u64,
    pub decoded_json_reserialized_sha256: String,
    pub oracle_witness: OracleWitness,
    pub representative_artifact: Option<String>,
}
struct JsonDigest {
    hasher: Sha256,
    bytes: u64,
}
impl std::io::Write for JsonDigest {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.hasher.update(bytes);
        self.bytes += bytes.len() as u64;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn decoded_json_digest(value: &impl Serialize) -> Result<(u64, String), String> {
    let mut output = JsonDigest {
        hasher: Sha256::new(),
        bytes: 0,
    };
    serde_json::to_writer(&mut output, value).map_err(|error| error.to_string())?;
    Ok((output.bytes, format!("{:x}", output.hasher.finalize())))
}
pub(super) fn validate_witness(witness: &OracleWitness) -> Result<(), String> {
    let (bytes, _) = decoded_json_digest(witness)?;
    if bytes > MAX_WITNESS_RESERIALIZED_BYTES {
        return Err(format!(
            "typed witness exceeds{} bytes",
            MAX_WITNESS_RESERIALIZED_BYTES
        ));
    }
    match witness {
        OracleWitness::Page { oracle, .. }
        | OracleWitness::File { oracle, .. }
        | OracleWitness::State { oracle, .. }
            if oracle.is_empty() =>
        {
            Err("specific actual oracle identity required".into())
        }
        OracleWitness::State { assertions, .. }
            if assertions.is_empty()
                || assertions
                    .iter()
                    .any(|assertion| assertion.actual != assertion.expected) =>
        {
            Err("nonempty passing typed oracle assertions required".into())
        }
        _ => Ok(()),
    }
}
/// The supplied oracle MUST validate the full actual decoded response before
/// producing a bounded typed witness. Size/hash/Value echo alone never proves
/// correctness. Raw failures and one representative/case are saved separately.
pub fn verify_and_summarize_response(
    trial: TrialName,
    original_id: &str,
    case: &str,
    decoded: &Value,
    oracle: impl FnOnce(&Value) -> Result<OracleWitness, String>,
    sink: &dyn ArtifactSink,
) -> Result<ResponseEvidence, String> {
    let witness = match oracle(decoded).and_then(|witness| {
        validate_witness(&witness)?;
        Ok(witness)
    }) {
        Ok(witness) => witness,
        Err(error) => {
            if let Err(io) = sink.raw_response(trial, original_id, case, true, decoded) {
                return Err(format!(
                    "oracle failure: {error}; raw failure artifact error: {io}"
                ));
            }
            return Err(error);
        }
    };
    let (bytes, sha256) = decoded_json_digest(decoded)?;
    let representative_artifact = sink.raw_response(trial, original_id, case, false, decoded)?;
    Ok(ResponseEvidence {
        decoded_json_reserialized_bytes: bytes,
        decoded_json_reserialized_sha256: sha256,
        oracle_witness: witness,
        representative_artifact,
    })
}
