// This is an example of a JavaScript file with a custom file extension.

function main() {
  // <block affects=":foo">
  console.log("Hi"); // Modified
  // </block>
}

function foo() {
  // <block name="foo">
  console.log("Hi from foo");
  // </block>
}
