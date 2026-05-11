# Primitives for "action-based" simulations

I'm building a set of simulation primitives inspired by state machines and the famous Gillespie algorithm for stochastic chemical kinetics simulations.

A state machine involves some entity traversing states in a deterministic or non-deterministic fashion like this traffic light:

```rust
#[derive(Debug, Clone, Copy, PartialEq)]
enum TrafficLight {
    Red,
    Yellow,
    Green,
}

impl TrafficLight {
    fn next(&self) -> Self {
        match self {
            TrafficLight::Red => TrafficLight::Green,
            TrafficLight::Green => TrafficLight::Yellow,
            TrafficLight::Yellow => TrafficLight::Red,
        }
    }

    fn can_go(&self) -> bool {
        matches!(self, TrafficLight::Green)
    }

    fn can_stop(&self) -> bool {
        matches!(self, TrafficLight::Red | TrafficLight::Yellow)
    }
}

fn main() {
    let mut light = TrafficLight::Red;
    
    println!("Starting state: {:?}", light);
    
    // Cycle through the states
    for _ in 0..6 {
        light = light.next();
        println!("State: {:?}, Can go: {}, Can stop: {}", 
                 light, light.can_go(), light.can_stop());
    }
}
```

A Gillespie simulation considers a system of chemical *reactions* acting on pools of distinct reactant species, with species counts defining the system state. The sequence of reaction events is stochastic. In a simulation realization, for each reaction, its intrinsic rate constant and the system state (via counts of reactant species) are used to pseudorandomly generate candidate reaction firing times from a probability distribution, thereby "scheduling" the next event. Hence, a scheduler (e.g., priority queue) keeping track of the forecasted next firing times for each reaction determines the next state change. Once the next reaction fires and the system changes state, only it and any *dependent* reactions (those for whom the reactant species pools were influenced by the reaction itself) require rescheduling -- other forecasted reaction firing times are independent and do not need to be resampled. This is essentially an optimization. 

Here's an example of a simple 4-reaction system:

```
1. transcription:       0       --kR--> mRNA
2. translation:         mRNA    --kP--> mRNA + protein
3. mRNA decay:          mRNA    --gR--> 0
4. protein decay:       protein --gP--> 0
```

In Gillespie framework, the state of the whole system (species counts of mRNA, protein) determines future state transitions, in a generalized way from the simple state machine above, since the state transitions occur through a collection of "reactions" as intermediaries competing to be the next one to fire. The fact that the scheduling of reactions uses pseudorandomness is only a detail and not a key part of this general state transition framework.

## Entities and actions

Here we want to generalize the concept of a reaction into a "schedulable action". I see this primitive as essentially being realized as an enum or struct that exposes a `schedule` method and returns the anticipated time of the action's next event given the current system state and time and assuming no changes in dependent state until that time. Scheduling must not mutate any state.

There must be some abstraction to define the system state at a given time. The assumption is that the model will consist of collections of entities that share property definitions, which could be thought of as a table where the rows correspond to entities and columns to state variables. There could be more than one type of entity, so potentially more than one such "table".

The state mutation logic when any action fires (a "step" of the simulation) is inevitably intertwined with the other entities and actions used in a given model, so it must be implemented globally and carefully by an author in a similar way to a classic state machine pattern: matching on the action that fired and on certain state variables to determine which entities require updating and updating them as needed. Ideally, state mutation after each step is complete behaves "reactively", such that at the end of an action's firing, the simulation engine can efficiently determine which entities were updated by the action and consequently reschedule all dependent actions, thereby keeping the system in a valid state. To close the loop on these dependencies, there must be a way to map entities to the actions that require their state for scheduling. Actions therefore may need to declare their dependent entities at creation time, but this is a design space to explore.

A simulation loop is therefore driven by:
* a global clock
* system state (entities)
* a priority queue popluated with actions and their next expected firing time
* the action firing callback
* an entity-to-action dependency map. 

At every step, the next action is determined from the queue, the clock is advanced, the action is fired and mutated entities are tracked, and finally dependent actions (including the one that just fired) are rescheduled. The historical state and timestamps of the simulation can be recorded as needed. 

With this architecture, a model author is responsible for defining action types and the state types that the actions act upon, and authoring the global firing callback consisting of state mutation logic caused by actions. To run a simulation, a user must populate/initialize the actions and state.


## Application

The application I have in mind is LEF simulation.

I have in mind a single 1D lattice, where some sites are CTCFs (and maybe other things). The state of the lattice does not change. The lefs have a pair of "legs" and these are the actual entities. There are two types of action:

* Step (left or right):  a single leg makes one step in a given direction
* Respawn: A pair of legs unbind and respawn at a single (non-CTCF) site

The legs themselves possess state:
* their position on the lattice
* their partner leg or shared lef identifier
* their intrinsic mobility status: moving, captured (by a ctcf), evicted (by a ctcf in the opposite orientation)

The legs also have an extrinsic mobility, which could be determined on the fly, based on whether a neighbor site is populated with another leg blocking it.

Steps:
* The scheduling of step actions depend on whether there is another leg blocking it in the direction it wishes to step (next time = inf) and otherwise potentially on the context of the lattice cell it is on (adding "friction").
* The firing of a step action will influence any rear-facing neighbor leg before the step and any posterior-facing neighbor leg after the step.

Respawns:
* The scheduling depends on the mobility status of both legs taken together -- different rules may be considered. If either leg gets "evicted", that should lead to an immediate respawn event (dt=0). If either is captured a longer lifetime is applied.
* The firing will depend on a custom rule about how a new spawn site is selected or sampled. The event will influence any neighbors of either leg before the action and any neighbors of the spawn site.

A CTCF has an intrinsic polarity, expressed by which side is its capture side:
* `captures_from: Right` → the CTCF's right side is its capture side; its left side evicts.
* `captures_from: Left` → the CTCF's left side is its capture side; its right side evicts.
* When a leg lands at the CTCF, the side it arrived from decides the outcome:
    * Arrival side matches `captures_from` → Captured (with probability p).
    * Arrival side is the opposite → Evicted (with probability p).
    * Otherwise → passes through (Moving) with probability 1 - p.
