# ephys-rs
electrophysiology software


## Super high level vision

- Adobe AfterEfects inspired graph interface with lots of window panes.
- In code, each node type is fairly self contained module, but the whole program is a single compliation unit (in the near term at least). This means we can avoid designing an ultra generic API, and can instead support a metaprogramming solution of some sort. 
- Use the same ui machinery as the open-ephys-plugin-camera, i think.
- Need a basic buffer and threading model that supports high throughput.
- Each node type exposes a certain number of inputs and outputs, each with some kind of requirements info. And then when you click on the connectors it highlights mathcing nodes already in the graph and also offers a list of supported types that could go there to create a new node.
- Visualiser nodes, recorder nodes, settings nodes, synchronisation nodes, source nodes ... they all work pretty much the same way.
- Support for raw recording and for spikes-only recording.


## Plan

- Do the basic GUI, with simmulated voltage and camera input nodes, band pass filters, gains, spikes.
- Switch in real camera, based on the open-ephys-plugin-camera.
- Switch in real acquisition.
- Record data.
- ...
