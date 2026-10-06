import "./app.css";

import { init } from "lefsim";
import { mount } from "svelte";

import App from "./App.svelte";

await init();
mount(App, { target: document.getElementById("app")! });
