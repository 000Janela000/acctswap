---------------------------- MODULE SwSwitch ----------------------------
(***************************************************************************)
(* `sw` switching the active Claude login while other writers touch it.   *)
(*                                                                         *)
(* The active login lives in two places written one after the other: the  *)
(* credential (tokens) and ~/.claude.json (the email). Other writers:     *)
(*   - another sw run (two tabs hitting a limit at once),                  *)
(*   - Claude Code renewing the active tokens (the old ones stop working), *)
(*     inside its own renewal lock,                                        *)
(*   - the user's /login (new tokens, then the email), with no lock.       *)
(*                                                                         *)
(* sw's protocol (crates/acctswap-core/src/switcher.rs):                        *)
(*   1. read email and tokens; ask the API who owns the tokens; if not the *)
(*      email, abort and retry (IdentityCheck);                            *)
(*   2. take Claude Code's renewal lock (HonorCcLock), re-read, and abort   *)
(*      and retry if anything changed since step 1;                        *)
(*   3. save the tokens under the email, write the target's tokens, then   *)
(*      the target's email; release.                                       *)
(* A whole sw run also holds sw's own lock (UseSwLock).                    *)
(*                                                                         *)
(* Each protection is a constant, so TLC can show both that the design    *)
(* holds and that removing any one protection breaks it. See README.md.   *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS Accounts, Procs, MaxVer, MaxRetries,
          UseSwLock, HonorCcLock, IdentityCheck,
          WithRefresh, WithLogin

VARIABLES
    credOwner, credVer,  \* the active credential: whose tokens, which version
    cfgEmail,            \* the account ~/.claude.json names
    latest,              \* latest[a]: the newest renewal of a's tokens; older ones are revoked
    saved,               \* saved[a]: sw's copy for a: [owner, ver]
    swLock, ccLock,      \* "free" or the holder
    pc, retries,         \* each sw run's step and retry count
    sEmail, sOwner, sVer, tgt,   \* each run's step-1 snapshot and chosen target
    loginPc, loginAcct           \* the user's /login

vars == <<credOwner, credVer, cfgEmail, latest, saved, swLock, ccLock, pc, retries,
          sEmail, sOwner, sVer, tgt, loginPc, loginAcct>>
runVars == <<pc, retries, sEmail, sOwner, sVer, tgt>>

A0 == CHOOSE a \in Accounts : TRUE

Init ==
    /\ credOwner = A0 /\ credVer = 1 /\ cfgEmail = A0
    /\ latest = [a \in Accounts |-> 1]
    /\ saved = [a \in Accounts |-> [owner |-> a, ver |-> 1]]
    /\ swLock = "free" /\ ccLock = "free"
    /\ pc = [p \in Procs |-> "start"] /\ retries = [p \in Procs |-> 0]
    /\ sEmail = [p \in Procs |-> A0] /\ sOwner = [p \in Procs |-> A0]
    /\ sVer = [p \in Procs |-> 1] /\ tgt = [p \in Procs |-> A0]
    /\ loginPc = "idle" /\ loginAcct = A0

\* Give up a run's locks and start it again (bounded, so the model stays finite).
Retry(p) ==
    /\ retries[p] < MaxRetries
    /\ retries' = [retries EXCEPT ![p] = @ + 1]
    /\ pc' = [pc EXCEPT ![p] = "start"]
    /\ swLock' = IF swLock = p THEN "free" ELSE swLock
    /\ ccLock' = IF ccLock = p THEN "free" ELSE ccLock

------------------------------------------------------------------------------
TakeSwLock(p) ==
    /\ pc[p] = "start"
    /\ IF UseSwLock THEN swLock = "free" /\ swLock' = p ELSE UNCHANGED swLock
    /\ pc' = [pc EXCEPT ![p] = "readEmail"]
    /\ UNCHANGED <<credOwner, credVer, cfgEmail, latest, saved, ccLock, retries,
                   sEmail, sOwner, sVer, tgt, loginPc, loginAcct>>

ReadEmail(p) ==
    /\ pc[p] = "readEmail"
    /\ sEmail' = [sEmail EXCEPT ![p] = cfgEmail]
    /\ pc' = [pc EXCEPT ![p] = "readCred"]
    /\ UNCHANGED <<credOwner, credVer, cfgEmail, latest, saved, swLock, ccLock, retries,
                   sOwner, sVer, tgt, loginPc, loginAcct>>

\* Read the tokens; with IdentityCheck, the API tells whose they are.
ReadCred(p) ==
    /\ pc[p] = "readCred"
    /\ sOwner' = [sOwner EXCEPT ![p] = credOwner]
    /\ sVer' = [sVer EXCEPT ![p] = credVer]
    /\ pc' = [pc EXCEPT ![p] = "check"]
    /\ UNCHANGED <<credOwner, credVer, cfgEmail, latest, saved, swLock, ccLock, retries,
                   sEmail, tgt, loginPc, loginAcct>>

Check(p) ==
    /\ pc[p] = "check"
    /\ IF IdentityCheck /\ sOwner[p] # sEmail[p]
       THEN /\ Retry(p)
            /\ UNCHANGED <<tgt>>
       ELSE /\ \E t \in Accounts \ {sEmail[p]} : tgt' = [tgt EXCEPT ![p] = t]
            /\ pc' = [pc EXCEPT ![p] = "takeCc"]
            /\ UNCHANGED <<swLock, ccLock, retries>>
    /\ UNCHANGED <<credOwner, credVer, cfgEmail, latest, saved, sEmail, sOwner, sVer,
                   loginPc, loginAcct>>

TakeCc(p) ==
    /\ pc[p] = "takeCc"
    /\ IF HonorCcLock THEN ccLock = "free" /\ ccLock' = p ELSE UNCHANGED ccLock
    /\ pc' = [pc EXCEPT ![p] = "recheck"]
    /\ UNCHANGED <<credOwner, credVer, cfgEmail, latest, saved, swLock, retries,
                   sEmail, sOwner, sVer, tgt, loginPc, loginAcct>>

\* Under the lock: commit only if nothing changed since the snapshot.
Recheck(p) ==
    /\ pc[p] = "recheck"
    /\ IF HonorCcLock /\ <<cfgEmail, credOwner, credVer>> # <<sEmail[p], sOwner[p], sVer[p]>>
       THEN Retry(p)
       ELSE /\ pc' = [pc EXCEPT ![p] = "save"]
            /\ UNCHANGED <<swLock, ccLock, retries>>
    /\ UNCHANGED <<credOwner, credVer, cfgEmail, latest, saved, sEmail, sOwner, sVer, tgt,
                   loginPc, loginAcct>>

Save(p) ==
    /\ pc[p] = "save"
    /\ saved' = [saved EXCEPT ![sEmail[p]] = [owner |-> sOwner[p], ver |-> sVer[p]]]
    /\ pc' = [pc EXCEPT ![p] = "writeCred"]
    /\ UNCHANGED <<credOwner, credVer, cfgEmail, latest, swLock, ccLock, retries,
                   sEmail, sOwner, sVer, tgt, loginPc, loginAcct>>

WriteCred(p) ==
    /\ pc[p] = "writeCred"
    /\ credOwner' = saved[tgt[p]].owner
    /\ credVer' = saved[tgt[p]].ver
    /\ pc' = [pc EXCEPT ![p] = "writeCfg"]
    /\ UNCHANGED <<cfgEmail, latest, saved, swLock, ccLock, retries,
                   sEmail, sOwner, sVer, tgt, loginPc, loginAcct>>

WriteCfg(p) ==
    /\ pc[p] = "writeCfg"
    /\ cfgEmail' = tgt[p]
    /\ pc' = [pc EXCEPT ![p] = "done"]
    /\ swLock' = IF swLock = p THEN "free" ELSE swLock
    /\ ccLock' = IF ccLock = p THEN "free" ELSE ccLock
    /\ UNCHANGED <<credOwner, credVer, latest, saved, retries,
                   sEmail, sOwner, sVer, tgt, loginPc, loginAcct>>

------------------------------------------------------------------------------
\* Claude Code renews the active tokens inside its renewal lock (one atomic step).
Refresh ==
    /\ WithRefresh
    /\ ccLock = "free"
    /\ latest[credOwner] < MaxVer
    /\ credVer = latest[credOwner]
    /\ latest' = [latest EXCEPT ![credOwner] = @ + 1]
    /\ credVer' = latest[credOwner] + 1
    /\ UNCHANGED <<credOwner, cfgEmail, saved, swLock, ccLock, runVars, loginPc, loginAcct>>

\* The user's /login: tokens of a new grant (revoking nothing), then the email. No lock.
LoginCred ==
    /\ WithLogin /\ loginPc = "idle"
    /\ \E a \in Accounts : loginAcct' = a /\ credOwner' = a /\ credVer' = latest[a]
    /\ loginPc' = "cfg"
    /\ UNCHANGED <<cfgEmail, latest, saved, swLock, ccLock, runVars>>

LoginCfg ==
    /\ loginPc = "cfg"
    /\ cfgEmail' = loginAcct
    /\ loginPc' = "done"
    /\ UNCHANGED <<credOwner, credVer, latest, saved, swLock, ccLock, runVars, loginAcct>>

Next ==
    \/ \E p \in Procs : TakeSwLock(p) \/ ReadEmail(p) \/ ReadCred(p) \/ Check(p)
                        \/ TakeCc(p) \/ Recheck(p) \/ Save(p) \/ WriteCred(p) \/ WriteCfg(p)
    \/ Refresh \/ LoginCred \/ LoginCfg

Spec == Init /\ [][Next]_vars

------------------------------------------------------------------------------
\* A saved copy only ever holds its own account's tokens.
IdentityOK == \A a \in Accounts : saved[a].owner = a

\* Saved copies of inactive accounts hold their newest (unrevoked) tokens, so switching
\* to them later works without a /login.
FreshSaves == \A a \in Accounts : a # credOwner => saved[a].ver = latest[a]

\* With no sw run and no /login in flight, tokens and email name the same account.
Quiet == /\ \A p \in Procs : pc[p] \in {"start", "done"}
         /\ loginPc \in {"idle", "done"}
ConsistentWhenQuiet == Quiet => credOwner = cfgEmail

\* Locks are only held by runs that are inside their critical steps.
LocksOK == \A p \in Procs : pc[p] \in {"start", "done"} => swLock # p /\ ccLock # p

TypeOK ==
    /\ credOwner \in Accounts /\ cfgEmail \in Accounts /\ credVer \in 1..MaxVer
    /\ \A a \in Accounts : latest[a] \in 1..MaxVer /\ saved[a].ver \in 1..MaxVer
=============================================================================
