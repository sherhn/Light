from common import module

MODULE = module(
    id="lisp",
    name="Lisp",
    category="Languages",
    summary="Common Lisp and Scheme: structural editing, a live REPL, compile and load, ASDF systems, symbol navigation and SBCL in Linux.",
    description="The Lisp module switches on everything Light does for Lisp: the reader with matching brackets and rainbow depth, structural editing (slurp, barf, splice, raise), formatting, completion and documentation from the live image, the REPL, run and compile of files, ASDF systems, go to definition, find usages and refactorings. SBCL runs on Windows or in your Linux through WSL.",
    builtin="lisp",
    languages=[],
    run=[],
    tools=[],
    detect=dict(extensions=["lisp", "lsp", "cl", "asd", "scm", "ss", "el", "clj", "cljs", "edn"], files=[]),
    templates=[
        dict(
            name="ASDF system", description="an ASDF system with src/, test/ and a README", open="src/main.lisp",
            files={
                "{{name}}.asd": '(asdf:defsystem "{{name}}"\n  :description "{{name}}"\n  :version "0.1.0"\n  :depends-on ()\n  :serial t\n  :components ((:module "src"\n                :components ((:file "package")\n                             (:file "main"))))\n  :in-order-to ((test-op (test-op "{{name}}/test"))))\n\n(asdf:defsystem "{{name}}/test"\n  :depends-on ("{{name}}")\n  :components ((:module "test"\n                :components ((:file "tests"))))\n  :perform (test-op (o c) (uiop:symbol-call :{{name}}/test :run)))\n',
                "src/package.lisp": "(defpackage :{{name}}\n  (:use :cl)\n  (:export #:main #:greet))\n",
                "src/main.lisp": '(in-package :{{name}})\n\n(defun greet (name)\n  "Return a greeting for NAME."\n  (format nil "Hello, ~a!" name))\n\n(defun main ()\n  (format t "~a~%" (greet "world")))\n',
                "test/tests.lisp": '(defpackage :{{name}}/test\n  (:use :cl)\n  (:export #:run))\n(in-package :{{name}}/test)\n\n(defun run ()\n  (assert (string= (funcall (intern "GREET" :{{name}}) "Lisp") "Hello, Lisp!"))\n  (format t "All tests passed.~%"))\n',
                "README.md": "# {{name}}\n\nA Common Lisp project created with Light.\n",
                ".gitignore": "*.fasl\n",
            },
        ),
    ],
)
