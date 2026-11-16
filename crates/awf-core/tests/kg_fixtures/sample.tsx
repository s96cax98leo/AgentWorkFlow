import React from "react";
import { Button } from "./button";

export function App() {
  return <Button onClick={() => track("x")} />;
}

function track(name: string) { console.log(name); }
