fruits = [
    # <block keep-sorted="asc"> This list is in order:
    'apple',
    'banana',
    'orange',
    # </block>
]

vegetables = [
    # <block keep-sorted="desc">
    'tomato',
    'lettuce',
    'potato',
    # </block>
]

items = [
    # <block keep-sorted="asc" keep-sorted-pattern="id: (?P<value>\d+)">
    "id: 1 apple",
    "id: 3 cherry",
    "id: 4 orange",
    # </block>
]

more_items = [
    # <block keep-sorted="asc" keep-sorted-pattern="id: (?P<value>\d+)">
    "id: 1 apple",
    "id: 3 cherry",
    "id: 10 orange",
    # </block>
]

defaults_unsorted = [
    # <block keep-sorted>
    'b',
    'a',
    'c'
    # </block>
]
