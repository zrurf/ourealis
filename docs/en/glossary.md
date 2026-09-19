# Glossary

The vocabulary Ourealis defines and uses. Terms are grouped by the part of the
system they belong to; within a group they are ordered as the pipeline meets them.

## Environment and cost

**Resistance feature vector** ($\mathbf{f} \in \mathbb{R}^D$)
The objective description of one map cell: surface type, traffic attributes,
direction constraint, crowding, and any further dimension the map declares. It
says what the ground *is*, not what it costs a particular runner. The map stores
these vectors and never a synthesised cost, which is what lets one map serve
every motion mode.

**Weight vector** ($\mathbf{w}$)
The runner's or the mode's preferences, one weight per feature dimension. Cost is
synthesised by combining the two at run time.

**Weight prior**
The per-mode default weight vector stored in the map (`WEIGHT_PRIOR`). A run may
override it, and may modulate it with attention gating.

**Attention gating**
An optional context-dependent modulation of the weight prior. A context vector is
projected into a query and the feature dimensions into keys; the softmax of their
scaled dot products gives a per-dimension gain, which is normalised to mean one,
clipped, renormalised and multiplied into the prior. The mechanism acts on
*feature channels*, not on a sequence — it is channel gating, and the projection
matrices are constructed from domain knowledge rather than trained.

**Cost per metre vs. equivalent metre**
Cost per metre (`*_cost_per_m`) is the local resistance of the ground, a
dimensionless multiplier on distance. An **equivalent metre** (`*_equiv_m`) is
the accumulated product of cost and distance along a path, so a route's cost is
expressed in the metres of ordinary ground it is worth. Route choice, connector
waiting times and search heuristics are all calibrated in equivalent metres.

**Hard constraint**
A rule that cannot be traded off: prohibited ground, a one-way link traversed
backwards. Hard constraints are not large costs; they are boolean conditions
enforced in the cost field ($+\infty$), in the search's line-of-sight check, in
the smoother's projection and in the lateral-offset feasibility check.

**Soft rule**
A preference that *can* be traded off, expressed either as a linear term
$\mathbf{w}^\top \mathbf{f}$ or as a multiplicative penalty. The two branches are
combined with a maximum, so neither can dilute the other.

**Mixed granularity**
The spatial representation rule: structured areas are described at the finest
resolution, open areas by coarse blocks that stand for a whole region, and
multi-level links not as area at all but as graph edges. The map's quadtree
skeleton records which areas are described at which granularity.

**Mean/max aggregation**
The rule that a coarse block stores both the mean and the maximum of the proxy
channel it covers. The mean describes the block; the maximum is what makes it
safe to search on, because an average would hide an obstacle inside an otherwise
open block.

**Proxy channel**
The single feature channel whose aggregate values are embedded in the quadtree
skeleton, declared once in the map's `AGGREGATION_RULES` together with its
quantisation. It is a summary for granularity decisions, not a cost.

**Drill hint**
A flag on a coarse block meaning "this block is not uniform; search must descend
to the fine cells before using it".

**Fingerprint**
A hash of the sources a derived layer was built from, recorded in that layer's
header. Loading recomputes it; a mismatch rejects the layer, so an edited map can
never be searched with a stale slope field, distance transform or roadmap.

**Derived layer**
A layer that can be rebuilt from others: slope from elevation, distance transform
from the hard mask, roadmap from features and constraints, path library from the
roadmap. Derived layers are caches, and every one of them carries a fingerprint.

## Spatial graph

**Fine grid cell**
A cell of the structured-area grid. A search node *is* its cell index; adjacency
is generated on demand, which is what keeps a large map's graph from being
materialised.

**Coarse block**
A certified-uniform region admitted from the map's quadtree skeleton, standing
for all the fine cells inside it. Admissible blocks replace those cells.

**Roadmap waypoint (PRM node)**
A point in an open area, connected to nearby waypoints that are mutually visible.
Waypoints model *desire lines*: the paths people take across open ground rather
than around its edges.

**Desire line**
An informal route across open space that is not marked as a path — a diagonal
crossing of a plaza, a worn line across a lawn. Ourealis represents them by
randomised roadmap waypoints, so different individuals can prefer slightly
different lines.

**Interface node**
A point on the boundary between open and structured ground that belongs to both
substrates: it attaches to the nearest roadmap waypoint and to the fine grid cell
it sits in. Interface nodes are what stitch the roadmap and the grid into one
connected graph.

**Z-axis link (connector)**
A multi-level connection — stair, lift, footbridge, underpass, predefined
circuit — represented as a graph edge rather than as ground. A link carries its
own three-dimensional length, unit cost, waiting time, permitted directions and
equivalent traversal speed. Its planar footprint is a region a flat line may not
cross, so a path must use the link.

**Link channels**
The two per-vertex channels a path carries for the links it traverses: an
elevation ramp and a speed ceiling. They are stamped on the finished path, so the
motion stage reads a stair's height and speed without knowing that links exist.

**Line of sight**
The condition for a search to connect two nodes directly. In a non-uniform field
it is not merely geometric: it walks the exact cells the segment enters, rejects
hard constraints and link footprints, and integrates the cost field along the
segment to price the edge.

**Cell traversal (Amanatides–Woo)**
The exact enumeration of the grid cells a segment passes through. It replaces
sampling along the segment, which cannot be made safe: a shortcut that clips one
corner of a building has a chord shorter than any fixed step, with both endpoints
on legal ground.

## Route planning

**Lazy Theta\***
The search algorithm. It connects a node to its grandparent whenever a line of
sight exists, producing any-angle paths rather than the eight-direction staircase
of grid A\*, and it defers the line-of-sight test to node expansion, which removes
most of the checks without losing the path quality.

**Turn penalty**
A small cost $\lambda_\theta (1 - \cos \Delta\theta)$ added to an edge for a change
of direction. It biases the search towards routes the motion stage can run
smoothly, and it is a preference rather than a constraint: it makes the search's
cost function non-metric, which is why search optimality is not claimed.

**Path-size factor** ($PS_j$)
A correction applied to a candidate's choice probability that discounts the parts
it shares with other candidates. Without it, two near-identical candidates
overlap-count one corridor and the choice model behaves as though that corridor
were twice as attractive.

**Logit temperature** ($\beta$, `beta_logit`)
The individual's rationality in route choice, in equivalent metres. A small
$\beta$ approaches a deterministic choice of the cheapest route; a large one
approaches a uniform draw. It is the main source of route diversity between
individuals, and it is an individual parameter, not a global setting.

**Candidate route**
One of up to $K$ distinct routes between two points, generated by searching,
penalising the edges of the accepted route and searching again. Candidates are
rejected when they duplicate an earlier one or overlap it beyond a threshold.

**Last-mile attach**
The contract for using a stored candidate route. A stored route's endpoints are
indexed by a coarse origin-destination key and can sit tens of metres from the
query, so it is used only when its exact endpoints are within the attach
threshold; the two connecting segments are then planned in a small window around
the endpoints and the joined route is re-priced. Only then is it eligible for the
choice model.

**Anchor**
A vertex the smoothing pass may not move. The endpoints of every route are
anchors, and so is every waypoint the caller requested: the band's contraction
otherwise pulls a waypoint's detour out of the route entirely.

**Anchor vertex**
See *Anchor*.

**Elastic band**
The smoothing method. Each interior vertex is pulled towards the midpoint of its
neighbours (contraction), pushed away from obstacles along the positive gradient
of the distance transform (avoidance), and then projected back onto feasible
ground. The projection, not the forces, is what keeps the result legal.

**Clearance ladder**
The validation chain that ends planning. Several passes touch the geometry after
the search — smoothing, resampling, simplification, corner rounding, spike
removal — and each can only check its own output, so the finished polyline is
validated against the exact cell traversal. If it fails, the path falls back
through the simplified, resampled, smoothed and finally searched geometry, and
only a complete failure is reported as an error.

**Spike**
An out-and-back of a metre or less: the path leaves a line and returns to it. The
motion stage advances arc length monotonically, so a spike makes the runner's
position reverse while their arc keeps growing. Spikes are removed wherever a
shortcut around them is traversable.

**Corner rounding**
The replacement of a sharp vertex by a circular arc of bounded radius, sampled so
that no inserted vertex turns by more than five degrees. The motion stage reads
curvature from three consecutive samples, so an unrounded vertex is a
zero-radius turn there whatever the adjacent segments look like.

## Motion

**Speed limit composition**
The per-sample ceiling built from the minimum of four terms: the physiological
slope limit, the curvature limit, the fatigue limit and the downhill braking cap.
A Z-axis link overrides all four with its own equivalent speed.

**Minetti model**
The physiological cost of running as a function of grade, per unit *horizontal*
distance:
$$C(i) = 155.4i^5 - 30.4i^4 - 43.3i^3 + 46.3i^2 + 19.5i + 3.6 \quad [\mathrm{J\,kg^{-1}\,m^{-1}}]$$
with $i$ clamped to $|i| \le 0.25$, beyond which the polynomial is not physical.
The speed limit follows from holding the energy rate constant.

**Look-ahead grade**
The grade a runner reacts to, aggregated over a window of 15–30 m ahead rather
than read at the current position. The default aggregation is a distance-weighted
mean, with an option to take the most adverse climb in the window, which models a
runner who is conservative about a visible hill.

**Downhill braking cap**
A ceiling of $k_{\text{down}} \cdot v_{\text{target}}$ applied whenever the
look-ahead grade is negative. Constant-energy running would predict a faster
descent; braking, landing impact and cadence say otherwise.

**Forward–backward sweep**
The two-pass construction of a speed profile. The forward pass limits
acceleration, the backward pass guarantees that deceleration begins early enough
for the next limit, and both use the same recurrence — the sign of the
acceleration term does not change with direction.

**Fixed-point iteration (time coupling)**
The loop that resolves the circular dependency between time and speed: fatigue
and pacing depend on elapsed time, elapsed time depends on the speed profile, and
the profile depends on fatigue and pacing. Each iteration evaluates the
time-dependent terms on the previous iteration's time map, rebuilds the profile,
and updates the total time with damping. A divergence check falls back to even
pacing if the iteration does not settle.

**Pacing strategy**
The normalised shape of intended speed over the run: even, positive split
(starting fast), or negative split (finishing fast).

**Critical-speed fatigue**
The model $v_{\text{fatigue}}(t) = v_{\text{crit}} + (v_0 - v_{\text{crit}})e^{-t/\tau_f}$,
which enters the speed limit as an upper bound: it can hold a tired runner back
but never forces a speed.

**Lateral offset**
The runner's sideways displacement from the centre line, a slowly drifting
random process with a preferred mean (the habitual side of the path). It is
applied along the *body* normal, not the path normal.

**Effective curvature** ($\kappa_{\text{eff}}$)
The curvature of the trajectory after the lateral offset is applied:
$$\kappa_{\text{eff}} = \frac{\kappa}{1 - d\kappa}$$
with $d$ the signed offset. Every downstream consumer of curvature — roll,
gyroscope, turn-rate statistics — uses this value; the centre-line curvature is
used only for the speed ceiling.

**Attitude**
The body orientation, decomposed as yaw, pitch and roll. Body yaw follows the
trajectory and is low-passed; head yaw leads it by a look-ahead time and is used
only for the gyroscope; pitch is the terrain grade plus a speed-proportional
forward lean; roll is the centripetal lean into a turn.

**Bounce**
The vertical oscillation of the centre of mass, one cycle per step. It is part of
the ground truth, not a sensor effect: the reported height, the barometric
altitude and the accelerometer's step harmonic all derive from it.

**Step harmonics**
The periodic content of the vertical acceleration at multiples of the step
frequency. The fundamental is not a free parameter — its amplitude and phase are
locked to the bounce — while the second and third harmonics are calibrated
against recordings. The second harmonic is largely produced by the asymmetry of
the bounce waveform, because differentiating a displacement harmonic amplifies it
by the square of its order.

**Phase reference** ($\phi_0$)
The single per-individual constant that anchors the bounce, the accelerometer's
step harmonics and the gyroscope's step oscillation. Sharing it is what makes a
height estimator that integrates acceleration agree with the barometer.

**Maneuver**
A behaviour the continuous speed profile cannot express, generated explicitly:
the standing start, the standing end, the dwell at a waypoint, and the on-the-spot
turn at a reversal.

**Reversal**
A point where the route requires the runner to face the opposite direction. It is
detected from the geometry, and the runner decelerates, turns on the spot with a
trapezoidal angular-velocity profile and accelerates away.

## Randomness and sensing

**Three-layer noise**
The separation of randomness by the process it acts on: decision noise (the Logit
draw between routes), motion noise (slow drift, step harmonics, high-frequency
position jitter) and sensor noise (per-device bias, white noise, region events).
The layers have different time scales and must not be conflated.

**Ornstein–Uhlenbeck process**
The model for every slowly drifting quantity — pace, lateral offset, sensor
biases. It is a mean-reverting random walk, and it is discretised exactly, so the
configured stationary standard deviation holds at any time step.

**Truth (ground truth)**
The sequence of exact states a run consists of: low-frequency centroid position
and its derivatives, the reported position including jitter, height above terrain,
attitude, effective curvature, grade, and the standing and turning flags. Every
sensor is a measurement of this one sequence.

**Low-frequency centroid vs. reported position**
Two positions are carried per sample. The low-frequency centroid is the smooth
motion that the accelerometer is built from; the reported position adds the
high-frequency jitter that a real trajectory file shows. Keeping them apart is
what prevents 3–5 cm of white jitter, differentiated twice at 100 Hz, from
appearing as several metres per second squared of force.

**Specific force**
What an accelerometer measures: $\mathbf{f} = \ddot{\mathbf{p}} - \mathbf{g}$ with
$\mathbf{g} = (0, 0, -9.81)$, so a device at rest reads $+9.81$ on its vertical
axis. The sign convention is defined once, in code and in this document.

**Region event**
A sensor disturbance triggered by the runner's position rather than by time:
GNSS multipath, GNSS dropout, magnetic disturbance. Each region carries the
event's probability, magnitude and duration parameters.

**Spatial-deterministic mode**
A region-event trigger mode in which the decision and the bias direction come
from a hash of `(seed, region, entry index)`. The same individual reproduces the
same event sequence on every run, which regression testing and parameter
calibration require; the alternative mode draws from the random stream.

**Multipath**
GNSS position error caused by reflected signals, modelled as a region-triggered
burst with a smooth envelope. It enters the reported position only, because a
static bias does not appear in a Doppler velocity.

**Correlated GNSS velocity**
Velocity noise derived from the same slow bias that perturbs the position, rather
than generated independently. Downstream filters expect the correlation, and
independent noise would make their innovations unrealistic.

## Evaluation and calibration

**Path ratio**
$L_{\text{path}} / d_{\text{euclid}}$, the route's length divided by the straight-line
distance between its endpoints. It is a coarse check that a route neither cuts
across everything nor wanders.

**Autocorrelation (ACF) of the residuals**
The colour of the motion noise. White noise decorrelates within one sample; a
drift process decays exponentially. The metric is defined with a fixed
*detrending window in seconds*, so it means the same thing at any sample rate.

**Lap-time consistency**
The coefficient of variation of lap durations over a multi-lap session. A real
runner varies by 1–4 %: much more means noise has leaked into the clock, much
less means the noise never reached the kinematics.

**Calibration knob**
A named parameter the calibration optimiser is allowed to move — cadence, bounce
amplitude and asymmetry, harmonics, target speed. Each carries physical bounds,
and each is applied to the individual's parameters rather than to a separate copy.

**Tolerance-normalised loss**
The calibration objective. Each observable has a reference value and a tolerance,
and the loss is the sum of squared deviations measured in tolerances, so one
observable that is off by a factor of five and another that is off by eight
percent are weighted by how precisely each is known, not by their relative error.

## Map format

**OMF (Ourealis Map Format)**
The static map container: one file holding environment descriptions, terrain,
derived caches and region annotations. It is read-only and immutable; changes are
made by rebuilding or patching.

**Layer**
A named array of chunks with one descriptor: identifier, kind (raster, bitmap,
graph, vector, region), channel count, element type, codec and quantisation.
Layer identifiers are grouped by class — terrain, features, constraints, regions,
graph, cache, extension.

**Chunk**
The unit of storage and of random access: a square block of one layer at one
level of detail, compressed independently. A missing chunk is a legal state
meaning "this area has no data at this granularity".

**Quantisation contract**
The single rule that recovers stored values: $\text{real} = \text{raw}\cdot\text{scale} + \text{bias}$,
with the scale and bias declared in the layer descriptor.

**Chunk directory**
The sorted array of records that maps a chunk's identity to its position in the
file. It is the addressing layer; the quadtree skeleton is the granularity layer.

**Quadtree skeleton**
The Morton-ordered node array that records which areas are described at which
granularity, together with each node's aggregate mean and maximum. It is small
enough to stay resident in memory.

**Morton key**
The interleaved coordinate that orders the skeleton and the chunks spatially. The
skeleton's key carries one depth bit above the coordinates, because a bare Morton
code would collide for the origin node at every depth.

**Tile reference**
The handle a skeleton node carries for the chunk that holds its aggregate data,
resolved through the directory.

**Origin-destination key (OD key)**
The coarse cell pair a stored candidate route is indexed by. It is an index, not
a coordinate: the route's exact endpoints are stored separately and are what the
last-mile attach contract compares against.

**Patch**
The incremental update format: a base-file hash, a list of replacement chunks, the
payloads and optional metadata replacements. Only source layers may be patched,
because replacing a derived layer would leave it inconsistent with its own
fingerprint.
