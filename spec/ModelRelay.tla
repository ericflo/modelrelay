----------------------------- MODULE ModelRelay -----------------------------
(***************************************************************************)
(* Formal TLA+ specification of the ModelRelay proxy-worker protocol       *)
(* (ProxyServerCore + WorkerSocketApp + ProxyHttpApp + WorkerDaemon).      *)
(*                                                                         *)
(* Models:                                                                 *)
(*   - Worker WebSocket connect, auth (valid/invalid), RegisterAck,        *)
(*     disconnect, crash, and wedged-event-loop states                     *)
(*   - Client HTTP request submission (unary & streaming), provider        *)
(*     queues (FIFO), ProviderRouting (only / ignore / allow_fallbacks),   *)
(*     multi-tenant owner_key_id scoping, and immediate queue draining     *)
(*   - Asynchronous per-worker FIFO WebSocket channels in both directions  *)
(*   - Streaming chunks, non-2xx streaming errors, mid-stream disconnects, *)
(*     requeue bounds, client cancellation + worker 499 ack + server reap  *)
(*   - Backend request error isolation vs sibling requests                 *)
(*   - Application-level JSON Ping/Pong heartbeats vs transport WS Pings   *)
(*   - Graceful worker shutdown drain                                      *)
(*                                                                         *)
(* Every protocol rule is controlled by a BOOLEAN constant.  In the real   *)
(* protocol every rule is TRUE.  Each `spec/Unsafe*.cfg` negative control  *)
(* sets exactly one rule to FALSE and must be refuted by TLC.              *)
(***************************************************************************)
EXTENDS Naturals, Sequences, FiniteSets

CONSTANTS
  Workers,               \* finite set of worker ids, e.g. {"w1", "w2"}
  Tenants,               \* finite set of tenant ids, e.g. {"t1", "t2"}
  Providers,             \* finite set of provider names, e.g. {"p1", "p2"}
  MaxConcurrent,         \* max concurrent requests per worker (>= 1)
  MaxReq,                \* max requests ever submitted
  MaxRequeue,            \* max requeues before failing a request
  MaxQueueLen,           \* max queued requests per provider
  \* ---- Scenario switches (enable feature dimensions per .cfg) ----
  AllowStreaming,        \* allow streaming requests and chunk emission
  AllowCrossProvider,    \* allow ProviderRouting (only/ignore/fallback)
  AllowMultiTenant,      \* allow multiple tenants submitting requests
  AllowGracefulShutdown, \* allow server-initiated graceful drain
  AllowHeartbeats,       \* allow JSON Ping/Pong, WS Ping, and wedged workers
  AllowAuthFailure,      \* allow worker auth rejection (Close 1008)
  AllowBackendError,     \* allow backend HTTP errors / non-2xx streaming errors
  AllowStrayFrames,      \* allow a connected worker to emit a stray Complete/Chunk
  \* ---- Protocol rules (all TRUE in the real protocol; 13 negative controls) ----
  AckCancelOrReap,                   \* F-01: worker acks Cancel with 499 AND server arms reap
  NoRequeueAfterChunk,               \* F-06: never requeue a stream after chunk 1 emitted
  VerifyOwnerBeforeComplete,         \* F-15: verify worker owns request before touching client
  IsolateBackendErrors,              \* F-02/F-15: single backend error never kills siblings/WS
  AtomicLoadBeforeComplete,          \* F-13: remove active request BEFORE sending ResponseComplete
  DrainOnRegisterAndDisconnect,      \* F-04: drain queued work on register & disconnect
  RequeueUnderRequestProvider,       \* F-05: requeue under request.provider, not worker.provider
  EnforceTenantIsolation,            \* F-09: tenant-scoped workers only serve matching tenant
  EnforceRoutingOnQueuedDispatch,    \* F-19: queued dispatch & fallback honor only/ignore
  HeartbeatIndependentOfWsPing,      \* F-03: WS Ping never resets/satisfies JSON heartbeat
  StopAssigningWhileDraining,        \* F-20: never dispatch new requests to draining workers
  RequireRegisterAckBeforeConnected, \* F-08/F-16: Connected only after RegisterAck
  UnaryNon2xxForStreamingError       \* F-10: non-2xx streaming backend errors send unary Complete

ReqIds == 1..MaxReq
None == "none"

VARIABLES
  \* ---- Worker-side state ----
  wState,         \* [w -> "offline" | "connecting" | "connected" | "auth_failed" | "wedged"]
  wAuthed,        \* [w -> BOOLEAN] whether this attempt used valid credentials
  wGotAck,        \* [w -> BOOLEAN] whether RegisterAck was received in this session
  wActive,        \* [w -> SUBSET ReqIds] requests actively executing on worker backend
  wPendingFinish, \* [w -> SUBSET ReqIds] requests sent Complete before active removal (F-13)
  wDraining,      \* [w -> BOOLEAN] worker received GracefulShutdown
  \* ---- Server-side worker state ----
  sConnected,     \* [w -> BOOLEAN] worker registered in ProxyServerCore
  sDraining,      \* [w -> BOOLEAN] worker marked draining on server
  sInFlight,      \* [w -> SUBSET ReqIds] server's worker.in_flight_requests
  sReportedLoad,  \* [w -> 0..MaxReq] server's worker.reported_load
  sAwaitingPong,  \* [w -> BOOLEAN] server sent JSON Ping and awaits JSON Pong
  \* ---- Server queues & requests ----
  sQueue,         \* [p \in Providers -> Seq(ReqIds)]
  nextReq,        \* 1..(MaxReq + 1)
  rState,         \* [r -> "new" | "queued" | "in_flight" | "completed" | "failed" | "cancelled"]
  rOwner,         \* [r -> Tenants]
  rProvider,      \* [r -> Providers]
  rStreaming,     \* [r -> BOOLEAN]
  rOnly,          \* [r -> Providers \cup {None}]
  rIgnore,        \* [r -> Providers \cup {None}]
  rAllowFallback, \* [r -> BOOLEAN]
  rWorker,        \* [r -> Workers \cup {None}]
  rCancelled,     \* [r -> BOOLEAN]
  rReapArmed,     \* [r -> BOOLEAN]
  rRequeues,      \* [r -> 0..MaxRequeue]
  rHasChunk,      \* [r -> BOOLEAN]
  \* ---- Client-visible & audit bookkeeping ----
  rClientChunks,   \* [r -> SUBSET Workers] workers from which client received stream chunks
  rClientStatus,   \* [r -> "none" | "200_sse" | "200_unary" | "400_error" | "502_error" | "503_unavail" | "sse_error"]
  rBackendNon2xx,  \* [r -> BOOLEAN] backend returned non-2xx HTTP status for this request
  rBadDelivery,    \* [r -> BOOLEAN] non-owner worker completed or chunked this request
  rSiblingAborted, \* [r -> BOOLEAN] healthy request aborted by sibling's backend error
  badStalePong,    \* BOOLEAN: Pong arrived with inflated load counting an already-completed req
  badDrainDispatch,\* BOOLEAN: request dispatched to a draining worker
  badWsPingStarve, \* BOOLEAN: transport WS Ping cleared heartbeat state of a wedged worker
  \* ---- Asynchronous per-worker FIFO WebSocket channels ----
  s2w,            \* [w -> Seq(S2WMsg)]
  w2s             \* [w -> Seq(W2SMsg)]

workerVars == <<wState, wAuthed, wGotAck, wActive, wPendingFinish, wDraining>>
serverWVars == <<sConnected, sDraining, sInFlight, sReportedLoad, sAwaitingPong>>
reqMetaVars == <<nextReq, rOwner, rProvider, rStreaming, rOnly, rIgnore, rAllowFallback>>
reqDynVars == <<sQueue, rState, rWorker, rCancelled, rReapArmed, rRequeues, rHasChunk>>
auditVars == <<rClientChunks, rClientStatus, rBackendNon2xx, rBadDelivery, rSiblingAborted,
               badStalePong, badDrainDispatch, badWsPingStarve>>
wireVars == <<s2w, w2s>>
vars == <<workerVars, serverWVars, reqMetaVars, reqDynVars, auditVars, wireVars>>

(* ------------------------------- Helpers -------------------------------- *)

Max(a, b) == IF a >= b THEN a ELSE b
Sub1(n) == IF n > 0 THEN n - 1 ELSE 0

DefaultTenant == CHOOSE t \in Tenants : TRUE
DefaultProvider == CHOOSE p \in Providers : TRUE

\* Deterministic assignment of each worker to a provider and tenant owner
\* according to the active scenario switches (avoids redundant symmetric copies).
WorkerProvider(w) ==
  IF AllowCrossProvider /\ w = "w2" /\ "p2" \in Providers THEN "p2"
  ELSE DefaultProvider

WorkerOwner(w) ==
  IF ~AllowMultiTenant THEN "shared"
  ELSE IF w = "w1" /\ "t1" \in Tenants THEN "t1"
  ELSE IF w = "w2" /\ "t2" \in Tenants THEN "t2"
  ELSE "shared"

SeqElems(s) == {s[i] : i \in 1..Len(s)}

RemoveFirst(s, x) ==
  IF x \notin SeqElems(s) THEN s
  ELSE LET i == CHOOSE k \in 1..Len(s) : s[k] = x
       IN  SubSeq(s, 1, i - 1) \o SubSeq(s, i + 1, Len(s))

SelectionLoad(inflightMap, rloadMap, w) ==
  Max(Cardinality(inflightMap[w]), rloadMap[w])

TenantAllowed(w, owner) ==
  \/ ~EnforceTenantIsolation
  \/ WorkerOwner(w) = "shared"
  \/ WorkerOwner(w) = owner

RoutingAllowedStrict(w, onlyP, ignoreP) ==
  /\ (ignoreP = None \/ WorkerProvider(w) # ignoreP)
  /\ (onlyP = None \/ WorkerProvider(w) = onlyP)

RoutingAllowedFallback(w, ignoreP, allowFb) ==
  /\ allowFb
  /\ (ignoreP = None \/ WorkerProvider(w) # ignoreP)

\* Whether worker w satisfies routing policy for request r (strict or fallback).
RoutingMatchesForReq(w, r) ==
  \/ RoutingAllowedStrict(w, rOnly[r], rIgnore[r])
  \/ RoutingAllowedFallback(w, rIgnore[r], rAllowFallback[r])

WorkerHasSlot(conn, drain, inflight, rload, w) ==
  /\ conn[w]
  /\ (~drain[w] \/ ~StopAssigningWhileDraining)
  /\ SelectionLoad(inflight, rload, w) < MaxConcurrent

\* Eligible workers for immediate dispatch of a request with given routing & tenant.
EligibleWorkersImmediate(conn, drain, inflight, rload, owner, prefProv, onlyP, ignoreP, allowFb) ==
  LET strict == {w \in Workers :
                   /\ WorkerHasSlot(conn, drain, inflight, rload, w)
                   /\ TenantAllowed(w, owner)
                   /\ RoutingAllowedStrict(w, onlyP, ignoreP)}
      strictPref == {w \in strict : WorkerProvider(w) = prefProv}
      fb == {w \in Workers :
               /\ WorkerHasSlot(conn, drain, inflight, rload, w)
               /\ TenantAllowed(w, owner)
               /\ IF EnforceRoutingOnQueuedDispatch
                    THEN RoutingAllowedFallback(w, ignoreP, allowFb)
                    ELSE allowFb}
      fbPref == {w \in fb : WorkerProvider(w) = prefProv}
  IN  IF strictPref # {} THEN strictPref
      ELSE IF strict # {} THEN strict
      ELSE IF fbPref # {} THEN fbPref
      ELSE fb

\* Whether worker w can dequeue queued request r (strictly checking routing if rule enabled).
CanDequeue(conn, drain, inflight, rload, w, r, ownerMap, onlyMap, ignoreMap, fbMap) ==
  /\ WorkerHasSlot(conn, drain, inflight, rload, w)
  /\ TenantAllowed(w, ownerMap[r])
  /\ IF EnforceRoutingOnQueuedDispatch
       THEN \/ RoutingAllowedStrict(w, onlyMap[r], ignoreMap[r])
            \/ RoutingAllowedFallback(w, ignoreMap[r], fbMap[r])
       ELSE TRUE

\* Deterministic re-insertion of requeued requests at front of target provider queues.
RECURSIVE EnqRequeued(_, _, _, _)
EnqRequeued(qs, rs, w, rProvMap) ==
  IF rs = {} THEN qs
  ELSE LET r == CHOOSE x \in rs : \A y \in rs : x <= y
           p == IF RequeueUnderRequestProvider THEN rProvMap[r] ELSE WorkerProvider(w)
       IN  EnqRequeued([qs EXCEPT ![p] = <<r>> \o @], rs \ {r}, w, rProvMap)

\* Deterministic queue drain across all providers & connected workers (mirrors
\* ProxyServerCore::dispatch_all_available executed atomically under core mutex).
RECURSIVE DrainAll(_, _, _, _, _, _, _, _, _, _, _, _, _, _)
DrainAll(conn, drain, inflight, rload, qs, rSt, rWk, s2wM, badDr, ownerM, onlyM, ignoreM, fbM, bound) ==
  LET queuedPairs ==
        {<<p, i, w>> \in Providers \X (1..MaxReq) \X Workers :
           /\ i <= Len(qs[p])
           /\ LET r == qs[p][i] IN
                /\ rSt[r] = "queued"
                /\ CanDequeue(conn, drain, inflight, rload, w, r, ownerM, onlyM, ignoreM, fbM)}
  IN  IF bound = 0 \/ queuedPairs = {}
      THEN [inflight |-> inflight, qs |-> qs, rSt |-> rSt, rWk |-> rWk,
            s2w |-> s2wM, badDr |-> badDr]
      ELSE \* Pick FIFO-earliest queued request for any ready worker
           LET pick == CHOOSE t \in queuedPairs :
                         \A u \in queuedPairs : t[2] <= u[2]
               p == pick[1]
               r == qs[p][pick[2]]
               w == pick[3]
               qs2 == [qs EXCEPT ![p] = RemoveFirst(@, r)]
               inf2 == [inflight EXCEPT ![w] = @ \cup {r}]
               st2 == [rSt EXCEPT ![r] = "in_flight"]
               wk2 == [rWk EXCEPT ![r] = w]
               s2w2 == [s2wM EXCEPT ![w] = Append(@, [type |-> "Req", r |-> r])]
               bd2 == badDr \/ drain[w]
           IN  DrainAll(conn, drain, inf2, rload, qs2, st2, wk2, s2w2, bd2,
                        ownerM, onlyM, ignoreM, fbM, bound - 1)

(* -------------------------------- Init ---------------------------------- *)

Init ==
  /\ wState = [w \in Workers |-> "offline"]
  /\ wAuthed = [w \in Workers |-> FALSE]
  /\ wGotAck = [w \in Workers |-> FALSE]
  /\ wActive = [w \in Workers |-> {}]
  /\ wPendingFinish = [w \in Workers |-> {}]
  /\ wDraining = [w \in Workers |-> FALSE]
  /\ sConnected = [w \in Workers |-> FALSE]
  /\ sDraining = [w \in Workers |-> FALSE]
  /\ sInFlight = [w \in Workers |-> {}]
  /\ sReportedLoad = [w \in Workers |-> 0]
  /\ sAwaitingPong = [w \in Workers |-> FALSE]
  /\ sQueue = [p \in Providers |-> <<>>]
  /\ nextReq = 1
  /\ rState = [r \in ReqIds |-> "new"]
  /\ rOwner = [r \in ReqIds |-> DefaultTenant]
  /\ rProvider = [r \in ReqIds |-> DefaultProvider]
  /\ rStreaming = [r \in ReqIds |-> FALSE]
  /\ rOnly = [r \in ReqIds |-> None]
  /\ rIgnore = [r \in ReqIds |-> None]
  /\ rAllowFallback = [r \in ReqIds |-> TRUE]
  /\ rWorker = [r \in ReqIds |-> None]
  /\ rCancelled = [r \in ReqIds |-> FALSE]
  /\ rReapArmed = [r \in ReqIds |-> FALSE]
  /\ rRequeues = [r \in ReqIds |-> 0]
  /\ rHasChunk = [r \in ReqIds |-> FALSE]
  /\ rClientChunks = [r \in ReqIds |-> {}]
  /\ rClientStatus = [r \in ReqIds |-> "none"]
  /\ rBackendNon2xx = [r \in ReqIds |-> FALSE]
  /\ rBadDelivery = [r \in ReqIds |-> FALSE]
  /\ rSiblingAborted = [r \in ReqIds |-> FALSE]
  /\ badStalePong = FALSE
  /\ badDrainDispatch = FALSE
  /\ badWsPingStarve = FALSE
  /\ s2w = [w \in Workers |-> <<>>]
  /\ w2s = [w \in Workers |-> <<>>]

(* ----------------------- Worker connect & auth -------------------------- *)

\* Worker initiates WebSocket connect & sends Register.
\* If RequireRegisterAckBeforeConnected = FALSE (F-08/F-16), desktop/worker
\* optimistically marks itself "connected" before auth/RegisterAck completes.
WorkerConnect(w) ==
  /\ wState[w] \in {"offline", "auth_failed"}
  /\ ~sConnected[w]
  /\ s2w[w] = <<>> /\ w2s[w] = <<>>
  /\ \E validSecret \in (IF AllowAuthFailure THEN {TRUE, FALSE} ELSE {TRUE}) :
       /\ wAuthed' = [wAuthed EXCEPT ![w] = validSecret]
       /\ wGotAck' = [wGotAck EXCEPT ![w] = FALSE]
       /\ wDraining' = [wDraining EXCEPT ![w] = FALSE]
       /\ wActive' = [wActive EXCEPT ![w] = {}]
       /\ wPendingFinish' = [wPendingFinish EXCEPT ![w] = {}]
       /\ wState' = [wState EXCEPT ![w] =
                       IF RequireRegisterAckBeforeConnected THEN "connecting" ELSE "connected"]
       /\ IF validSecret
            THEN \* Server accepts auth, registers worker, sends RegisterAck, drains queue
                 LET conn2 == [sConnected EXCEPT ![w] = TRUE]
                     drain2 == [sDraining EXCEPT ![w] = FALSE]
                     inf2 == [sInFlight EXCEPT ![w] = {}]
                     rl2 == [sReportedLoad EXCEPT ![w] = 0]
                     s2wAck == [s2w EXCEPT ![w] = Append(@, [type |-> "RegAck"])]
                 IN  IF DrainOnRegisterAndDisconnect
                       THEN LET d == DrainAll(conn2, drain2, inf2, rl2, sQueue, rState, rWorker,
                                              s2wAck, badDrainDispatch, rOwner, rOnly, rIgnore,
                                              rAllowFallback, MaxReq)
                            IN  /\ sConnected' = conn2
                                /\ sDraining' = drain2
                                /\ sInFlight' = d.inflight
                                /\ sReportedLoad' = rl2
                                /\ sQueue' = d.qs
                                /\ rState' = d.rSt
                                /\ rWorker' = d.rWk
                                /\ s2w' = d.s2w
                                /\ badDrainDispatch' = d.badDr
                       ELSE /\ sConnected' = conn2
                            /\ sDraining' = drain2
                            /\ sInFlight' = inf2
                            /\ sReportedLoad' = rl2
                            /\ s2w' = s2wAck
                            /\ UNCHANGED <<sQueue, rState, rWorker, badDrainDispatch>>
            ELSE \* Server rejects auth with Close 1008
                 /\ s2w' = [s2w EXCEPT ![w] = Append(@, [type |-> "AuthReject"])]
                 /\ UNCHANGED <<sConnected, sDraining, sInFlight, sReportedLoad,
                                sQueue, rState, rWorker, badDrainDispatch>>
       /\ sAwaitingPong' = [sAwaitingPong EXCEPT ![w] = FALSE]
  /\ UNCHANGED <<reqMetaVars, rCancelled, rReapArmed, rRequeues, rHasChunk,
                 rClientChunks, rClientStatus, rBackendNon2xx, rBadDelivery,
                 rSiblingAborted, badStalePong, badWsPingStarve, w2s>>

\* Worker receives RegisterAck or AuthReject from server.
WorkerRecvHandshake(w) ==
  /\ Len(s2w[w]) > 0
  /\ Head(s2w[w]).type \in {"RegAck", "AuthReject"}
  /\ LET msg == Head(s2w[w]) IN
       /\ s2w' = [s2w EXCEPT ![w] = Tail(@)]
       /\ IF msg.type = "RegAck"
            THEN /\ wGotAck' = [wGotAck EXCEPT ![w] = TRUE]
                 /\ wState' = [wState EXCEPT ![w] = "connected"]
            ELSE /\ wGotAck' = [wGotAck EXCEPT ![w] = FALSE]
                 /\ wState' = [wState EXCEPT ![w] = "auth_failed"]
  /\ UNCHANGED <<wAuthed, wActive, wPendingFinish, wDraining, serverWVars,
                 reqMetaVars, reqDynVars, auditVars, w2s>>

(* ------------------------- Request submission --------------------------- *)

SubmitRequest ==
  /\ nextReq <= MaxReq
  /\ LET r == nextReq
         tenants == IF AllowMultiTenant THEN Tenants ELSE {DefaultTenant}
         provs == IF AllowCrossProvider THEN Providers ELSE {DefaultProvider}
         streamOpts == IF AllowStreaming THEN {FALSE, TRUE} ELSE {FALSE}
         onlyOpts == IF AllowCrossProvider THEN Providers \cup {None} ELSE {None}
         ignoreOpts == IF AllowCrossProvider THEN Providers \cup {None} ELSE {None}
         fbOpts == IF AllowCrossProvider THEN {FALSE, TRUE} ELSE {TRUE}
     IN  \E owner \in tenants, prov \in provs, isStream \in streamOpts,
            onlyP \in onlyOpts, ignoreP \in ignoreOpts, allowFb \in fbOpts :
           /\ nextReq' = nextReq + 1
           /\ rOwner' = [rOwner EXCEPT ![r] = owner]
           /\ rProvider' = [rProvider EXCEPT ![r] = prov]
           /\ rStreaming' = [rStreaming EXCEPT ![r] = isStream]
           /\ rOnly' = [rOnly EXCEPT ![r] = onlyP]
           /\ rIgnore' = [rIgnore EXCEPT ![r] = ignoreP]
           /\ rAllowFallback' = [rAllowFallback EXCEPT ![r] = allowFb]
           /\ LET elig == EligibleWorkersImmediate(sConnected, sDraining, sInFlight, sReportedLoad,
                                                   owner, prov, onlyP, ignoreP, allowFb)
              IN  IF elig # {}
                    THEN \E w \in elig :
                           /\ rState' = [rState EXCEPT ![r] = "in_flight"]
                           /\ rWorker' = [rWorker EXCEPT ![r] = w]
                           /\ sInFlight' = [sInFlight EXCEPT ![w] = @ \cup {r}]
                           /\ s2w' = [s2w EXCEPT ![w] = Append(@, [type |-> "Req", r |-> r])]
                           /\ badDrainDispatch' = (badDrainDispatch \/ sDraining[w])
                           /\ UNCHANGED <<sQueue, rClientStatus>>
                    ELSE IF Len(sQueue[prov]) < MaxQueueLen
                           THEN /\ rState' = [rState EXCEPT ![r] = "queued"]
                                /\ rWorker' = [rWorker EXCEPT ![r] = None]
                                /\ sQueue' = [sQueue EXCEPT ![prov] = Append(@, r)]
                                /\ UNCHANGED <<sInFlight, s2w, badDrainDispatch, rClientStatus>>
                           ELSE \* Queue full -> reject immediately with 503
                                /\ rState' = [rState EXCEPT ![r] = "failed"]
                                /\ rWorker' = [rWorker EXCEPT ![r] = None]
                                /\ rClientStatus' = [rClientStatus EXCEPT ![r] = "503_unavail"]
                                /\ UNCHANGED <<sInFlight, s2w, badDrainDispatch, sQueue>>
  /\ UNCHANGED <<workerVars, sConnected, sDraining, sReportedLoad, sAwaitingPong,
                 rCancelled, rReapArmed, rRequeues, rHasChunk,
                 rClientChunks, rBackendNon2xx, rBadDelivery, rSiblingAborted,
                 badStalePong, badWsPingStarve, w2s>>

(* -------------------- Worker execution & responses ---------------------- *)

\* Worker receives a Request frame from server.
WorkerRecvRequest(w) ==
  /\ wState[w] = "connected"
  /\ Len(s2w[w]) > 0
  /\ Head(s2w[w]).type = "Req"
  /\ LET r == Head(s2w[w]).r IN
       /\ s2w' = [s2w EXCEPT ![w] = Tail(@)]
       /\ IF wDraining[w]
            THEN \* Worker already draining rejects new request with Error
                 /\ w2s' = [w2s EXCEPT ![w] = Append(@, [type |-> "Error", r |-> r])]
                 /\ UNCHANGED wActive
            ELSE /\ wActive' = [wActive EXCEPT ![w] = @ \cup {r}]
                 /\ UNCHANGED w2s
  /\ UNCHANGED <<wState, wAuthed, wGotAck, wPendingFinish, wDraining,
                 serverWVars, reqMetaVars, reqDynVars, auditVars>>

\* Worker streams a 2xx chunk for an active streaming request.
\* Each worker emits at most one chunk per streaming request (enough to commit
\* 200 SSE on the client and detect cross-worker Franken-streams on requeue).
WorkerSendChunk(w) ==
  /\ wState[w] = "connected"
  /\ \E r \in wActive[w] :
       /\ rStreaming[r]
       /\ ~rBackendNon2xx[r]
       /\ w \notin rClientChunks[r]
       /\ ~\E i \in 1..Len(w2s[w]) : w2s[w][i].type = "Chunk" /\ w2s[w][i].r = r
       /\ w2s' = [w2s EXCEPT ![w] = Append(@, [type |-> "Chunk", r |-> r, isErr |-> FALSE])]
  /\ UNCHANGED <<workerVars, serverWVars, reqMetaVars, reqDynVars, auditVars, s2w>>

\* Worker finishes an active request normally (200 OK).
\* When AtomicLoadBeforeComplete = TRUE, r is removed from wActive atomically.
\* When AtomicLoadBeforeComplete = FALSE (F-13), r stays in wActive/wPendingFinish
\* briefly so an interleaved Ping/Pong can report stale load!
WorkerCompleteOk(w) ==
  /\ wState[w] = "connected"
  /\ \E r \in wActive[w] \ wPendingFinish[w] :
       /\ w2s' = [w2s EXCEPT ![w] = Append(@, [type |-> "Complete", r |-> r, status |-> "200"])]
       /\ IF AtomicLoadBeforeComplete
            THEN /\ wActive' = [wActive EXCEPT ![w] = @ \ {r}]
                 /\ UNCHANGED wPendingFinish
            ELSE /\ wPendingFinish' = [wPendingFinish EXCEPT ![w] = @ \cup {r}]
                 /\ UNCHANGED wActive
  /\ UNCHANGED <<wState, wAuthed, wGotAck, wDraining, serverWVars,
                 reqMetaVars, reqDynVars, auditVars, s2w>>

\* Delayed removal of completed request when AtomicLoadBeforeComplete = FALSE (F-13).
WorkerFinishDeferredRemove(w) ==
  /\ ~AtomicLoadBeforeComplete
  /\ wState[w] = "connected"
  /\ \E r \in wPendingFinish[w] :
       /\ wActive' = [wActive EXCEPT ![w] = @ \ {r}]
       /\ wPendingFinish' = [wPendingFinish EXCEPT ![w] = @ \ {r}]
  /\ UNCHANGED <<wState, wAuthed, wGotAck, wDraining, serverWVars,
                 reqMetaVars, reqDynVars, auditVars, wireVars>>

\* Local backend returns a non-2xx HTTP error (e.g. 400) for a streaming request (F-10).
\* When UnaryNon2xxForStreamingError = TRUE, worker sends unary Complete(400) without Chunk.
\* When UnaryNon2xxForStreamingError = FALSE, worker erroneously emits Chunk then Complete(400).
WorkerBackendNon2xxStream(w) ==
  /\ AllowBackendError /\ AllowStreaming
  /\ wState[w] = "connected"
  /\ \E r \in wActive[w] \ wPendingFinish[w] :
       /\ rStreaming[r] /\ ~rHasChunk[r] /\ ~rBackendNon2xx[r]
       /\ ~\E i \in 1..Len(w2s[w]) : w2s[w][i].type = "Chunk" /\ w2s[w][i].r = r
       /\ rBackendNon2xx' = [rBackendNon2xx EXCEPT ![r] = TRUE]
       /\ IF UnaryNon2xxForStreamingError
            THEN /\ wActive' = [wActive EXCEPT ![w] = @ \ {r}]
                 /\ w2s' = [w2s EXCEPT ![w] = Append(@, [type |-> "Complete", r |-> r, status |-> "400"])]
            ELSE \* Buggy old path: streams non-2xx error body as ResponseChunk!
                 /\ wActive' = [wActive EXCEPT ![w] = @ \ {r}]
                 /\ w2s' = [w2s EXCEPT ![w] =
                              Append(Append(@, [type |-> "Chunk", r |-> r, isErr |-> TRUE]),
                                     [type |-> "Complete", r |-> r, status |-> "400"])]
  /\ UNCHANGED <<wState, wAuthed, wGotAck, wPendingFinish, wDraining, serverWVars,
                 reqMetaVars, reqDynVars, rClientChunks, rClientStatus, rBadDelivery,
                 rSiblingAborted, badStalePong, badDrainDispatch, badWsPingStarve, s2w>>

\* Local backend connection fails (e.g. TCP reset / UTF-8 crash in old code) on one request (F-02/F-15).
\* When IsolateBackendErrors = TRUE, worker removes ONLY r and sends Error(r).
\* When IsolateBackendErrors = FALSE, worker aborts ALL sibling requests and tears down WS!
WorkerBackendFailure(w) ==
  /\ AllowBackendError
  /\ wState[w] = "connected"
  /\ \E r \in wActive[w] \ wPendingFinish[w] :
       IF IsolateBackendErrors
         THEN /\ wActive' = [wActive EXCEPT ![w] = @ \ {r}]
              /\ w2s' = [w2s EXCEPT ![w] = Append(@, [type |-> "Error", r |-> r])]
              /\ UNCHANGED <<wState, wGotAck, wPendingFinish, rSiblingAborted>>
         ELSE \* Buggy old behavior: aborts every sibling active request and drops session!
              LET siblings == wActive[w] \ {r} IN
              /\ wActive' = [wActive EXCEPT ![w] = {}]
              /\ wPendingFinish' = [wPendingFinish EXCEPT ![w] = {}]
              /\ wState' = [wState EXCEPT ![w] = "offline"]
              /\ wGotAck' = [wGotAck EXCEPT ![w] = FALSE]
              /\ rSiblingAborted' = [x \in ReqIds |-> rSiblingAborted[x] \/ (x \in siblings)]
              /\ UNCHANGED w2s
  /\ UNCHANGED <<wAuthed, wDraining, serverWVars, reqMetaVars, reqDynVars,
                 rClientChunks, rClientStatus, rBackendNon2xx, rBadDelivery,
                 badStalePong, badDrainDispatch, badWsPingStarve, s2w>>

\* A connected worker emits a stray/late Complete for an active request it does NOT own (F-15).
WorkerSendStrayComplete(w) ==
  /\ AllowStrayFrames
  /\ wState[w] = "connected"
  /\ \E r \in ReqIds :
       /\ rState[r] = "in_flight"
       /\ rWorker[r] # w
       /\ ~\E i \in 1..Len(w2s[w]) : w2s[w][i].type = "Complete" /\ w2s[w][i].r = r
       /\ w2s' = [w2s EXCEPT ![w] = Append(@, [type |-> "Complete", r |-> r, status |-> "200"])]
  /\ UNCHANGED <<workerVars, serverWVars, reqMetaVars, reqDynVars, auditVars, s2w>>

(* ---------------- Client cancellation & server reap --------------------- *)

\* Client disconnects or times out on a queued or in-flight request.
ClientCancelRequest(r) ==
  /\ rState[r] \in {"queued", "in_flight"}
  /\ ~rCancelled[r]
  /\ IF rState[r] = "queued"
       THEN \* Remove from rProvider[r]'s queue (or fail to find it if orphaned under wrong provider!)
            /\ sQueue' = [sQueue EXCEPT ![rProvider[r]] = RemoveFirst(@, r)]
            /\ rState' = [rState EXCEPT ![r] = "cancelled"]
            /\ rCancelled' = [rCancelled EXCEPT ![r] = TRUE]
            /\ UNCHANGED <<rReapArmed, s2w>>
       ELSE LET w == rWorker[r] IN
            /\ rCancelled' = [rCancelled EXCEPT ![r] = TRUE]
            /\ rReapArmed' = [rReapArmed EXCEPT ![r] = AckCancelOrReap]
            /\ s2w' = [s2w EXCEPT ![w] = Append(@, [type |-> "Cancel", r |-> r])]
            /\ UNCHANGED <<sQueue, rState>>
  /\ UNCHANGED <<workerVars, serverWVars, reqMetaVars, rWorker, rRequeues, rHasChunk,
                 auditVars, w2s>>

\* Worker receives Cancel(r), aborts local task, and (when AckCancelOrReap = TRUE)
\* immediately sends ResponseComplete(499) so the server frees the concurrency slot.
WorkerRecvCancel(w) ==
  /\ wState[w] = "connected"
  /\ Len(s2w[w]) > 0
  /\ Head(s2w[w]).type = "Cancel"
  /\ LET r == Head(s2w[w]).r IN
       /\ s2w' = [s2w EXCEPT ![w] = Tail(@)]
       /\ wActive' = [wActive EXCEPT ![w] = @ \ {r}]
       /\ wPendingFinish' = [wPendingFinish EXCEPT ![w] = @ \ {r}]
       /\ IF AckCancelOrReap
            THEN w2s' = [w2s EXCEPT ![w] = Append(@, [type |-> "Complete", r |-> r, status |-> "499"])]
            ELSE UNCHANGED w2s
  /\ UNCHANGED <<wState, wAuthed, wGotAck, wDraining, serverWVars,
                 reqMetaVars, reqDynVars, auditVars>>

\* Server maintenance loop reaps an unacknowledged cancelled request after grace period (F-01).
ServerReapCancelled(r) ==
  /\ AckCancelOrReap
  /\ rState[r] = "in_flight"
  /\ rCancelled[r] /\ rReapArmed[r]
  /\ LET w == rWorker[r]
         inf1 == [sInFlight EXCEPT ![w] = @ \ {r}]
         drainDone == sDraining[w] /\ inf1[w] = {}
         rl1 == IF drainDone THEN [sReportedLoad EXCEPT ![w] = 0]
                ELSE [sReportedLoad EXCEPT ![w] = Sub1(@)]
         conn1 == IF drainDone THEN [sConnected EXCEPT ![w] = FALSE] ELSE sConnected
         drain1 == IF drainDone THEN [sDraining EXCEPT ![w] = FALSE] ELSE sDraining
         s2w1 == IF drainDone THEN [s2w EXCEPT ![w] = <<>>] ELSE s2w
         w2s1 == IF drainDone THEN [w2s EXCEPT ![w] = <<>>] ELSE w2s
         rSt1 == [rState EXCEPT ![r] = "cancelled"]
         rWk1 == [rWorker EXCEPT ![r] = None]
         d == DrainAll(conn1, drain1, inf1, rl1, sQueue, rSt1, rWk1, s2w1,
                       badDrainDispatch, rOwner, rOnly, rIgnore, rAllowFallback, MaxReq)
     IN  /\ sConnected' = conn1
         /\ sDraining' = drain1
         /\ sInFlight' = d.inflight
         /\ sReportedLoad' = rl1
         /\ sAwaitingPong' = IF drainDone THEN [sAwaitingPong EXCEPT ![w] = FALSE] ELSE sAwaitingPong
         /\ sQueue' = d.qs
         /\ rState' = d.rSt
         /\ rWorker' = d.rWk
         /\ rReapArmed' = [rReapArmed EXCEPT ![r] = FALSE]
         /\ s2w' = d.s2w
         /\ w2s' = w2s1
         /\ badDrainDispatch' = d.badDr
         /\ IF drainDone
              THEN /\ wState' = [wState EXCEPT ![w] = "offline"]
                   /\ wAuthed' = [wAuthed EXCEPT ![w] = FALSE]
                   /\ wGotAck' = [wGotAck EXCEPT ![w] = FALSE]
                   /\ wActive' = [wActive EXCEPT ![w] = {}]
                   /\ wPendingFinish' = [wPendingFinish EXCEPT ![w] = {}]
                   /\ wDraining' = [wDraining EXCEPT ![w] = FALSE]
              ELSE UNCHANGED workerVars
  /\ UNCHANGED <<reqMetaVars, rCancelled, rRequeues, rHasChunk,
                 rClientChunks, rClientStatus, rBackendNon2xx, rBadDelivery,
                 rSiblingAborted, badStalePong, badWsPingStarve>>

(* -------------------- Server handles worker frames ---------------------- *)

\* Server processes Chunk, Complete, or Error from worker w's FIFO w2s channel.
ServerRecvWorkerResponse(w) ==
  /\ sConnected[w]
  /\ Len(w2s[w]) > 0
  /\ Head(w2s[w]).type \in {"Chunk", "Complete", "Error"}
  /\ LET msg == Head(w2s[w])
         r == msg.r
         w2sTail == [w2s EXCEPT ![w] = Tail(@)]
         isOwner == rState[r] = "in_flight" /\ rWorker[r] = w
     IN  CASE msg.type = "Chunk" ->
           /\ w2s' = w2sTail
           /\ IF isOwner /\ ~rCancelled[r]
                THEN /\ rHasChunk' = [rHasChunk EXCEPT ![r] = TRUE]
                     /\ rClientChunks' = [rClientChunks EXCEPT ![r] = @ \cup {w}]
                     /\ rClientStatus' = [rClientStatus EXCEPT ![r] = "200_sse"]
                     /\ UNCHANGED rBadDelivery
                ELSE IF ~VerifyOwnerBeforeComplete /\ rState[r] = "in_flight"
                       THEN /\ rBadDelivery' = [rBadDelivery EXCEPT ![r] = TRUE]
                            /\ UNCHANGED <<rHasChunk, rClientChunks, rClientStatus>>
                       ELSE UNCHANGED <<rHasChunk, rClientChunks, rClientStatus, rBadDelivery>>
           /\ UNCHANGED <<workerVars, serverWVars, reqMetaVars, sQueue, rState, rWorker,
                          rCancelled, rReapArmed, rRequeues, rBackendNon2xx,
                          rSiblingAborted, badStalePong, badDrainDispatch, badWsPingStarve, s2w>>

         [] msg.type = "Complete" ->
           IF isOwner
             THEN LET inf1 == [sInFlight EXCEPT ![w] = @ \ {r}]
                      drainDone == sDraining[w] /\ inf1[w] = {}
                      rl1 == IF drainDone THEN [sReportedLoad EXCEPT ![w] = 0]
                             ELSE [sReportedLoad EXCEPT ![w] = Sub1(@)]
                      conn1 == IF drainDone THEN [sConnected EXCEPT ![w] = FALSE] ELSE sConnected
                      drain1 == IF drainDone THEN [sDraining EXCEPT ![w] = FALSE] ELSE sDraining
                      s2w1 == IF drainDone THEN [s2w EXCEPT ![w] = <<>>] ELSE s2w
                      w2s1 == IF drainDone THEN [w2s EXCEPT ![w] = <<>>] ELSE w2sTail
                      termState == IF rCancelled[r] \/ msg.status = "499" THEN "cancelled" ELSE "completed"
                      newStatus == IF termState = "cancelled" THEN rClientStatus[r]
                                   ELSE IF rHasChunk[r] THEN rClientStatus[r]
                                   ELSE IF msg.status = "400" THEN "400_error"
                                   ELSE "200_unary"
                      rSt1 == [rState EXCEPT ![r] = termState]
                      rWk1 == [rWorker EXCEPT ![r] = None]
                      d == DrainAll(conn1, drain1, inf1, rl1, sQueue, rSt1, rWk1, s2w1,
                                    badDrainDispatch, rOwner, rOnly, rIgnore, rAllowFallback, MaxReq)
                  IN  /\ sConnected' = conn1
                      /\ sDraining' = drain1
                      /\ sInFlight' = d.inflight
                      /\ sReportedLoad' = rl1
                      /\ sAwaitingPong' = IF drainDone THEN [sAwaitingPong EXCEPT ![w] = FALSE] ELSE sAwaitingPong
                      /\ sQueue' = d.qs
                      /\ rState' = d.rSt
                      /\ rWorker' = d.rWk
                      /\ rReapArmed' = [rReapArmed EXCEPT ![r] = FALSE]
                      /\ rClientStatus' = [rClientStatus EXCEPT ![r] = newStatus]
                      /\ s2w' = d.s2w
                      /\ w2s' = w2s1
                      /\ badDrainDispatch' = d.badDr
                      /\ IF drainDone
                           THEN /\ wState' = [wState EXCEPT ![w] = "offline"]
                                /\ wAuthed' = [wAuthed EXCEPT ![w] = FALSE]
                                /\ wGotAck' = [wGotAck EXCEPT ![w] = FALSE]
                                /\ wActive' = [wActive EXCEPT ![w] = {}]
                                /\ wPendingFinish' = [wPendingFinish EXCEPT ![w] = {}]
                                /\ wDraining' = [wDraining EXCEPT ![w] = FALSE]
                           ELSE UNCHANGED workerVars
                      /\ UNCHANGED <<reqMetaVars, rCancelled, rRequeues, rHasChunk,
                                     rClientChunks, rBackendNon2xx, rBadDelivery,
                                     rSiblingAborted, badStalePong, badWsPingStarve>>
             ELSE \* Non-owner Complete!
                  /\ w2s' = w2sTail
                  /\ IF ~VerifyOwnerBeforeComplete /\ rState[r] = "in_flight"
                       THEN \* Buggy old server removed pending sender before checking ownership!
                            /\ rBadDelivery' = [rBadDelivery EXCEPT ![r] = TRUE]
                            /\ UNCHANGED <<workerVars, serverWVars, reqMetaVars, reqDynVars,
                                           rClientChunks, rClientStatus, rBackendNon2xx,
                                           rSiblingAborted, badStalePong, badDrainDispatch,
                                           badWsPingStarve, s2w>>
                       ELSE \* Safe: benignly drop stale/non-owner Complete
                            UNCHANGED <<workerVars, serverWVars, reqMetaVars, reqDynVars,
                                        auditVars, s2w>>

         [] msg.type = "Error" ->
           IF isOwner
             THEN LET inf1 == [sInFlight EXCEPT ![w] = @ \ {r}]
                      drainDone == sDraining[w] /\ inf1[w] = {}
                      rl1 == IF drainDone THEN [sReportedLoad EXCEPT ![w] = 0]
                             ELSE [sReportedLoad EXCEPT ![w] = Sub1(@)]
                      conn1 == IF drainDone THEN [sConnected EXCEPT ![w] = FALSE] ELSE sConnected
                      drain1 == IF drainDone THEN [sDraining EXCEPT ![w] = FALSE] ELSE sDraining
                      s2w1 == IF drainDone THEN [s2w EXCEPT ![w] = <<>>] ELSE s2w
                      w2s1 == IF drainDone THEN [w2s EXCEPT ![w] = <<>>] ELSE w2sTail
                      newStatus == IF rHasChunk[r] THEN "sse_error" ELSE "502_error"
                      rSt1 == [rState EXCEPT ![r] = "failed"]
                      rWk1 == [rWorker EXCEPT ![r] = None]
                      d == DrainAll(conn1, drain1, inf1, rl1, sQueue, rSt1, rWk1, s2w1,
                                    badDrainDispatch, rOwner, rOnly, rIgnore, rAllowFallback, MaxReq)
                  IN  /\ sConnected' = conn1
                      /\ sDraining' = drain1
                      /\ sInFlight' = d.inflight
                      /\ sReportedLoad' = rl1
                      /\ sAwaitingPong' = IF drainDone THEN [sAwaitingPong EXCEPT ![w] = FALSE] ELSE sAwaitingPong
                      /\ sQueue' = d.qs
                      /\ rState' = d.rSt
                      /\ rWorker' = d.rWk
                      /\ rReapArmed' = [rReapArmed EXCEPT ![r] = FALSE]
                      /\ rClientStatus' = [rClientStatus EXCEPT ![r] = newStatus]
                      /\ s2w' = d.s2w
                      /\ w2s' = w2s1
                      /\ badDrainDispatch' = d.badDr
                      /\ IF drainDone
                           THEN /\ wState' = [wState EXCEPT ![w] = "offline"]
                                /\ wAuthed' = [wAuthed EXCEPT ![w] = FALSE]
                                /\ wGotAck' = [wGotAck EXCEPT ![w] = FALSE]
                                /\ wActive' = [wActive EXCEPT ![w] = {}]
                                /\ wPendingFinish' = [wPendingFinish EXCEPT ![w] = {}]
                                /\ wDraining' = [wDraining EXCEPT ![w] = FALSE]
                           ELSE UNCHANGED workerVars
                      /\ UNCHANGED <<reqMetaVars, rCancelled, rRequeues, rHasChunk,
                                     rClientChunks, rBackendNon2xx, rBadDelivery,
                                     rSiblingAborted, badStalePong, badWsPingStarve>>
             ELSE /\ w2s' = w2sTail
                  /\ UNCHANGED <<workerVars, serverWVars, reqMetaVars, reqDynVars, auditVars, s2w>>

(* ---------------- Heartbeats, WS Pings & wedged workers ----------------- *)

\* Connected worker's JSON event loop wedges (cannot process JSON Ping, but WS transport stays up).
WorkerWedge(w) ==
  /\ AllowHeartbeats
  /\ wState[w] = "connected" /\ sConnected[w]
  /\ wState' = [wState EXCEPT ![w] = "wedged"]
  /\ UNCHANGED <<wAuthed, wGotAck, wActive, wPendingFinish, wDraining,
                 serverWVars, reqMetaVars, reqDynVars, auditVars, wireVars>>

\* Server sends application-level JSON Ping on heartbeat interval tick.
ServerSendHeartbeatPing(w) ==
  /\ AllowHeartbeats
  /\ sConnected[w] /\ ~sAwaitingPong[w]
  /\ ~\E i \in 1..Len(s2w[w]) : s2w[w][i].type = "Ping"
  /\ sAwaitingPong' = [sAwaitingPong EXCEPT ![w] = TRUE]
  /\ s2w' = [s2w EXCEPT ![w] = Append(@, [type |-> "Ping"])]
  /\ UNCHANGED <<workerVars, sConnected, sDraining, sInFlight, sReportedLoad,
                 reqMetaVars, reqDynVars, auditVars, w2s>>

\* Healthy (non-wedged) worker replies to JSON Ping with JSON Pong carrying active load.
WorkerRecvPing(w) ==
  /\ wState[w] = "connected"
  /\ Len(s2w[w]) > 0 /\ Head(s2w[w]).type = "Ping"
  /\ LET load == Cardinality(wActive[w])
         isStale == wPendingFinish[w] # {}
     IN  /\ s2w' = [s2w EXCEPT ![w] = Tail(@)]
         /\ w2s' = [w2s EXCEPT ![w] = Append(@, [type |-> "Pong", load |-> load, stale |-> isStale])]
  /\ UNCHANGED <<workerVars, serverWVars, reqMetaVars, reqDynVars, auditVars>>

\* Server receives JSON Pong from worker w.
ServerRecvPong(w) ==
  /\ sConnected[w]
  /\ Len(w2s[w]) > 0 /\ Head(w2s[w]).type = "Pong"
  /\ LET msg == Head(w2s[w])
         rl1 == [sReportedLoad EXCEPT ![w] = msg.load]
         d == DrainAll(sConnected, sDraining, sInFlight, rl1, sQueue, rState, rWorker,
                       s2w, badDrainDispatch, rOwner, rOnly, rIgnore, rAllowFallback, MaxReq)
     IN  /\ w2s' = [w2s EXCEPT ![w] = Tail(@)]
         /\ sAwaitingPong' = [sAwaitingPong EXCEPT ![w] = FALSE]
         /\ sReportedLoad' = rl1
         /\ sInFlight' = d.inflight
         /\ sQueue' = d.qs
         /\ rState' = d.rSt
         /\ rWorker' = d.rWk
         /\ s2w' = d.s2w
         /\ badDrainDispatch' = d.badDr
         /\ badStalePong' = (badStalePong \/ msg.stale)
  /\ UNCHANGED <<workerVars, sConnected, sDraining, reqMetaVars,
                 rCancelled, rReapArmed, rRequeues, rHasChunk,
                 rClientChunks, rClientStatus, rBackendNon2xx, rBadDelivery,
                 rSiblingAborted, badWsPingStarve>>

\* Server receives a transport-level WS Ping from worker w (every 15s in code; F-03).
\* Modeled out-of-band of the JSON w2s stream because WS control frames bypass JSON framing.
\* When HeartbeatIndependentOfWsPing = TRUE, transport WS Ping does NOT touch sAwaitingPong.
\* When FALSE (old bug), it clears sAwaitingPong and starves wedged-worker detection!
ServerRecvWsPing(w) ==
  /\ AllowHeartbeats
  /\ sConnected[w] /\ sAwaitingPong[w]
  /\ wState[w] \in {"connected", "wedged"}
  /\ IF HeartbeatIndependentOfWsPing
       THEN UNCHANGED <<sAwaitingPong, badWsPingStarve>>
       ELSE /\ sAwaitingPong' = [sAwaitingPong EXCEPT ![w] = FALSE]
            /\ badWsPingStarve' = (badWsPingStarve \/ wState[w] = "wedged")
  /\ UNCHANGED <<workerVars, sConnected, sDraining, sInFlight, sReportedLoad,
                 reqMetaVars, reqDynVars, rClientChunks, rClientStatus, rBackendNon2xx,
                 rBadDelivery, rSiblingAborted, badStalePong, badDrainDispatch, wireVars>>

(* ---------------- Graceful shutdown & worker disconnect ----------------- *)

\* Server initiates graceful shutdown on all currently connected non-draining workers.
ServerBeginGracefulShutdown ==
  /\ AllowGracefulShutdown
  /\ \E w \in Workers : sConnected[w] /\ ~sDraining[w]
  /\ LET targets == {w \in Workers : sConnected[w] /\ ~sDraining[w]}
         idleTargets == {w \in targets : sInFlight[w] = {}}
         busyTargets == targets \ idleTargets
     IN  /\ sDraining' = [w \in Workers |-> (sDraining[w] \/ (w \in busyTargets)) /\ (w \notin idleTargets)]
         /\ sConnected' = [w \in Workers |-> sConnected[w] /\ (w \notin idleTargets)]
         /\ sReportedLoad' = [w \in Workers |-> IF w \in idleTargets THEN 0 ELSE sReportedLoad[w]]
         /\ sAwaitingPong' = [w \in Workers |-> IF w \in idleTargets THEN FALSE ELSE sAwaitingPong[w]]
         /\ s2w' = [w \in Workers |->
                      IF w \in idleTargets THEN <<>>
                      ELSE IF w \in busyTargets THEN Append(s2w[w], [type |-> "Shutdown"])
                      ELSE s2w[w]]
         /\ w2s' = [w \in Workers |-> IF w \in idleTargets THEN <<>> ELSE w2s[w]]
         /\ wState' = [w \in Workers |-> IF w \in idleTargets THEN "offline" ELSE wState[w]]
         /\ wAuthed' = [w \in Workers |-> IF w \in idleTargets THEN FALSE ELSE wAuthed[w]]
         /\ wGotAck' = [w \in Workers |-> IF w \in idleTargets THEN FALSE ELSE wGotAck[w]]
         /\ wActive' = [w \in Workers |-> IF w \in idleTargets THEN {} ELSE wActive[w]]
         /\ wPendingFinish' = [w \in Workers |-> IF w \in idleTargets THEN {} ELSE wPendingFinish[w]]
         /\ wDraining' = [w \in Workers |-> IF w \in idleTargets THEN FALSE ELSE wDraining[w]]
  /\ UNCHANGED <<sInFlight, reqMetaVars, reqDynVars, auditVars>>

WorkerRecvShutdown(w) ==
  /\ wState[w] = "connected"
  /\ Len(s2w[w]) > 0 /\ Head(s2w[w]).type = "Shutdown"
  /\ s2w' = [s2w EXCEPT ![w] = Tail(@)]
  /\ wDraining' = [wDraining EXCEPT ![w] = TRUE]
  /\ UNCHANGED <<wState, wAuthed, wGotAck, wActive, wPendingFinish,
                 serverWVars, reqMetaVars, reqDynVars, auditVars, w2s>>

\* Helper: apply disconnect cleanup for worker w (used for TCP drop and heartbeat timeout).
DisconnectWorkerEffect(w) ==
  LET lost == sInFlight[w]
      requeueable == {r \in lost :
                        /\ ~rCancelled[r]
                        /\ (~rHasChunk[r] \/ ~NoRequeueAfterChunk)
                        /\ rRequeues[r] < MaxRequeue}
      failedMidStream == {r \in lost : ~rCancelled[r] /\ rHasChunk[r] /\ NoRequeueAfterChunk}
      failedCancelled == {r \in lost : rCancelled[r]}
      failedExhausted == {r \in lost : ~rCancelled[r] /\ (~rHasChunk[r] \/ ~NoRequeueAfterChunk)
                                       /\ rRequeues[r] >= MaxRequeue}
      conn1 == [sConnected EXCEPT ![w] = FALSE]
      drain1 == [sDraining EXCEPT ![w] = FALSE]
      inf1 == [sInFlight EXCEPT ![w] = {}]
      rl1 == [sReportedLoad EXCEPT ![w] = 0]
      s2w1 == [s2w EXCEPT ![w] = <<>>]
      w2s1 == [w2s EXCEPT ![w] = <<>>]
      \* Re-enqueue requeueable requests under rProvider[r] (or buggy WorkerProvider(w) for F-05!)
      qs1 == EnqRequeued(sQueue, requeueable, w, rProvider)
      rSt1 == [r \in ReqIds |->
                 IF r \in requeueable THEN "queued"
                 ELSE IF r \in failedCancelled THEN "cancelled"
                 ELSE IF r \in (failedMidStream \cup failedExhausted) THEN "failed"
                 ELSE rState[r]]
      rWk1 == [r \in ReqIds |-> IF r \in lost THEN None ELSE rWorker[r]]
      rReq1 == [r \in ReqIds |-> IF r \in requeueable THEN rRequeues[r] + 1 ELSE rRequeues[r]]
      rStStatus1 == [r \in ReqIds |->
                       IF r \in failedMidStream THEN "sse_error"
                       ELSE IF r \in failedExhausted THEN "503_unavail"
                       ELSE rClientStatus[r]]
  IN  /\ wState' = [wState EXCEPT ![w] = "offline"]
      /\ wAuthed' = [wAuthed EXCEPT ![w] = FALSE]
      /\ wGotAck' = [wGotAck EXCEPT ![w] = FALSE]
      /\ wActive' = [wActive EXCEPT ![w] = {}]
      /\ wPendingFinish' = [wPendingFinish EXCEPT ![w] = {}]
      /\ wDraining' = [wDraining EXCEPT ![w] = FALSE]
      /\ sConnected' = conn1
      /\ sDraining' = drain1
      /\ sReportedLoad' = rl1
      /\ sAwaitingPong' = [sAwaitingPong EXCEPT ![w] = FALSE]
      /\ w2s' = w2s1
      /\ rRequeues' = rReq1
      /\ rReapArmed' = [r \in ReqIds |-> IF r \in lost THEN FALSE ELSE rReapArmed[r]]
      /\ rClientStatus' = rStStatus1
      /\ IF DrainOnRegisterAndDisconnect
           THEN LET d == DrainAll(conn1, drain1, inf1, rl1, qs1, rSt1, rWk1, s2w1,
                                  badDrainDispatch, rOwner, rOnly, rIgnore,
                                  rAllowFallback, MaxReq)
                IN  /\ sInFlight' = d.inflight
                    /\ sQueue' = d.qs
                    /\ rState' = d.rSt
                    /\ rWorker' = d.rWk
                    /\ s2w' = d.s2w
                    /\ badDrainDispatch' = d.badDr
           ELSE /\ sInFlight' = inf1
                /\ sQueue' = qs1
                /\ rState' = rSt1
                /\ rWorker' = rWk1
                /\ s2w' = s2w1
                /\ UNCHANGED badDrainDispatch

\* Worker disconnects abruptly (crash/network drop) and server observes TCP close.
WorkerDisconnect(w) ==
  /\ sConnected[w]
  /\ DisconnectWorkerEffect(w)
  /\ UNCHANGED <<reqMetaVars, rCancelled, rHasChunk,
                 rClientChunks, rBackendNon2xx, rBadDelivery, rSiblingAborted,
                 badStalePong, badWsPingStarve>>

\* Server heartbeat pong timeout disconnects a wedged/unresponsive worker (F-03).
ServerHeartbeatTimeout(w) ==
  /\ AllowHeartbeats
  /\ sConnected[w] /\ sAwaitingPong[w]
  /\ wState[w] = "wedged"
  /\ DisconnectWorkerEffect(w)
  /\ UNCHANGED <<reqMetaVars, rCancelled, rHasChunk,
                 rClientChunks, rBackendNon2xx, rBadDelivery, rSiblingAborted,
                 badStalePong, badWsPingStarve>>

\* Server expires a queued request whose queue timeout elapsed.
ServerExpireQueueTimeout(p) ==
  /\ Len(sQueue[p]) > 0
  /\ LET r == Head(sQueue[p]) IN
       /\ sQueue' = [sQueue EXCEPT ![p] = Tail(@)]
       /\ rState' = [rState EXCEPT ![r] = "failed"]
       /\ rClientStatus' = [rClientStatus EXCEPT ![r] = "503_unavail"]
  /\ UNCHANGED <<workerVars, serverWVars, reqMetaVars, rWorker, rCancelled,
                 rReapArmed, rRequeues, rHasChunk,
                 rClientChunks, rBackendNon2xx, rBadDelivery, rSiblingAborted,
                 badStalePong, badDrainDispatch, badWsPingStarve, wireVars>>

(* ------------------------------ Next ------------------------------------ *)

Next ==
  \/ SubmitRequest
  \/ ServerBeginGracefulShutdown
  \/ \E r \in ReqIds :
       \/ ClientCancelRequest(r)
       \/ ServerReapCancelled(r)
  \/ \E p \in Providers :
       ServerExpireQueueTimeout(p)
  \/ \E w \in Workers :
       \/ WorkerConnect(w)
       \/ WorkerRecvHandshake(w)
       \/ WorkerRecvRequest(w)
       \/ WorkerSendChunk(w)
       \/ WorkerCompleteOk(w)
       \/ WorkerFinishDeferredRemove(w)
       \/ WorkerBackendNon2xxStream(w)
       \/ WorkerBackendFailure(w)
       \/ WorkerSendStrayComplete(w)
       \/ WorkerRecvCancel(w)
       \/ WorkerRecvShutdown(w)
       \/ WorkerWedge(w)
       \/ WorkerRecvPing(w)
       \/ WorkerDisconnect(w)
       \/ ServerRecvWorkerResponse(w)
       \/ ServerSendHeartbeatPing(w)
       \/ ServerRecvPong(w)
       \/ ServerRecvWsPing(w)
       \/ ServerHeartbeatTimeout(w)

Spec == Init /\ [][Next]_vars

\* Fairness spec for Liveness.cfg: progress actions (delivery, completion,
\* cancellation/reap, heartbeat timeout, queue expiry, graceful drain) make progress.
FairSpec ==
  /\ Spec
  /\ \A w \in Workers :
       /\ WF_vars(WorkerRecvHandshake(w))
       /\ WF_vars(WorkerRecvRequest(w))
       /\ WF_vars(WorkerCompleteOk(w))
       /\ WF_vars(WorkerRecvCancel(w))
       /\ WF_vars(WorkerRecvShutdown(w))
       /\ WF_vars(WorkerRecvPing(w))
       /\ WF_vars(ServerRecvWorkerResponse(w))
       /\ WF_vars(ServerSendHeartbeatPing(w))
       /\ WF_vars(ServerRecvPong(w))
       /\ WF_vars(ServerHeartbeatTimeout(w))
  /\ \A r \in ReqIds :
       WF_vars(ServerReapCancelled(r))
  /\ \A p \in Providers :
       WF_vars(ServerExpireQueueTimeout(p))

(* --------------------------- Safety Invariants -------------------------- *)

\* 1. Capacity never exceeded on any worker.
CapacityRespected ==
  \A w \in Workers : Cardinality(sInFlight[w]) <= MaxConcurrent

\* 2. F-01: Every server slot on a live worker is backed by either a Request on
\*    the wire, an active backend task, a terminal reply on the wire, or an
\*    armed server cancel-reap timer. No slot can leak permanently.
NoOrphanedServerSlot ==
  \A w \in Workers :
    (sConnected[w] /\ wState[w] = "connected") =>
      \A r \in sInFlight[w] :
        \/ \E i \in 1..Len(s2w[w]) : s2w[w][i].type = "Req" /\ s2w[w][i].r = r
        \/ r \in wActive[w]
        \/ \E i \in 1..Len(w2s[w]) :
             w2s[w][i].type \in {"Complete", "Error"} /\ w2s[w][i].r = r
        \/ (rCancelled[r] /\ rReapArmed[r])

\* 3. F-06: No Franken-streams — client-visible chunks for a request come from
\*    at most one worker.
SingleStreamOrigin ==
  \A r \in ReqIds : Cardinality(rClientChunks[r]) <= 1

\* 4. F-15: No non-owner worker can deliver a chunk or steal completion of an
\*    active request owned by another worker.
OwnerMatchesDelivery ==
  \A r \in ReqIds : ~rBadDelivery[r]

\* 5. F-02/F-15: A backend failure on one request never aborts healthy sibling
\*    requests on the same worker.
ErrorIsolation ==
  \A r \in ReqIds : ~rSiblingAborted[r]

\* 6. F-13: Worker never sends a Pong reporting inflated load for a request
\*    whose ResponseComplete was already placed ahead of it on the socket.
LoadAccountingAccurate ==
  ~badStalePong

\* 7. F-04: Work-conserving dispatch — no queued request is compatible with a
\*    connected, non-draining worker that has free capacity.
LegalWorkerForReq(w, r) ==
  /\ sConnected[w]
  /\ ~sDraining[w]
  /\ SelectionLoad(sInFlight, sReportedLoad, w) < MaxConcurrent
  /\ (WorkerOwner(w) = "shared" \/ WorkerOwner(w) = rOwner[r])
  /\ RoutingMatchesForReq(w, r)

WorkConserving ==
  \A r \in ReqIds :
    rState[r] = "queued" => ~\E w \in Workers : LegalWorkerForReq(w, r)

\* 8. F-05: Every queued request resides in its own rProvider[r] queue, and
\*    every entry in sQueue[p] is queued with rProvider = p.
QueueProviderConsistent ==
  /\ \A r \in ReqIds : rState[r] = "queued" => r \in SeqElems(sQueue[rProvider[r]])
  /\ \A p \in Providers :
       \A i \in 1..Len(sQueue[p]) :
         LET r == sQueue[p][i] IN rState[r] = "queued" /\ rProvider[r] = p

\* 9. F-09: Multi-tenant isolation — no request is ever in-flight on a worker
\*    owned by a different tenant.
TenantIsolated ==
  \A r \in ReqIds :
    rState[r] = "in_flight" =>
      LET w == rWorker[r] IN
        w \in Workers /\ (WorkerOwner(w) = "shared" \/ WorkerOwner(w) = rOwner[r])

\* 10. F-19: ProviderRouting (ignore & only/allow_fallbacks) holds for every
\*     in-flight request, whether dispatched immediately or from the queue.
RoutingRespected ==
  \A r \in ReqIds :
    rState[r] = "in_flight" =>
      LET w == rWorker[r] IN
        w \in Workers /\ RoutingMatchesForReq(w, r)

\* 11. F-03: Transport-level WS Ping never clears a wedged worker's JSON heartbeat.
WedgedWorkerDetected ==
  ~badWsPingStarve

\* 12. F-20: No new request is ever dispatched to a worker marked as draining.
NoDispatchWhileDraining ==
  ~badDrainDispatch

\* 13. F-08/F-16: A worker reports "connected" only after authenticating and
\*     receiving RegisterAck from the server.
ConnectedMeansRegistered ==
  \A w \in Workers : wState[w] = "connected" => (wGotAck[w] /\ wAuthed[w])

\* 14. F-10: A non-2xx streaming backend error never commits HTTP 200 SSE to the client.
ErrorStatusPreserved ==
  \A r \in ReqIds : rBackendNon2xx[r] => rClientStatus[r] # "200_sse"

(* ------------------------- Temporal Liveness ---------------------------- *)

\* Every submitted request eventually reaches a terminal state (no permanent hang).
AllRequestsTerminate ==
  \A r \in ReqIds :
    (rState[r] \in {"queued", "in_flight"}) ~> (rState[r] \in {"completed", "failed", "cancelled"})

\* Every wedged worker is eventually disconnected by the server heartbeat timeout.
AllWedgedWorkersReaped ==
  \A w \in Workers :
    (wState[w] = "wedged" /\ sConnected[w]) ~> (~sConnected[w])

\* Every draining worker eventually disconnects once in-flight work finishes.
AllDrainingWorkersExit ==
  \A w \in Workers :
    (sConnected[w] /\ sDraining[w]) ~> (~sConnected[w])

=============================================================================
